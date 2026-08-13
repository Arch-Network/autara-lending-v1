use std::collections::HashMap;

use anyhow::Context;
use apl_token::state::GenericTokenAccount;
use arch_sdk::{
    arch_program::pubkey::Pubkey, AccountFilter, AccountInfo, AccountInfoWithPubkey,
    AsyncArchRpcClient,
};
use bytemuck::Pod;

use crate::token_mint::TokenMint;

pub struct GetMultipleAccountsBatch {
    pub accounts: Vec<Vec<Option<AccountInfoWithPubkey>>>,
}

impl GetMultipleAccountsBatch {
    pub fn iter(&self) -> impl Iterator<Item = &AccountInfoWithPubkey> {
        self.accounts
            .iter()
            .flat_map(|chunk| chunk.iter().flatten())
    }

    pub fn into_iter(self) -> impl Iterator<Item = AccountInfoWithPubkey> {
        self.accounts
            .into_iter()
            .flat_map(|chunk| chunk.into_iter().flatten())
    }
}

#[async_trait::async_trait]
pub trait ArchAsyncRpcExt {
    async fn get_multiple_accounts_batch(
        &self,
        pubkeys: &[Pubkey],
    ) -> anyhow::Result<GetMultipleAccountsBatch>;
    async fn get_program_accounts_with<T: Send>(
        &self,
        program_id: &Pubkey,
        filters: Option<Vec<AccountFilter>>,
        map: impl FnMut(AccountInfo) -> Option<T> + Send + Sync,
    ) -> anyhow::Result<impl Iterator<Item = (Pubkey, T)>>;
    async fn get_pod_account<T: Pod>(&self, key: &Pubkey) -> anyhow::Result<T>;
    async fn get_mints(&self, pubkeys: &[Pubkey]) -> anyhow::Result<HashMap<Pubkey, TokenMint>> {
        self.get_multiple_accounts_batch(pubkeys)
            .await?
            .iter()
            .map(|acc| {
                TokenMint::try_from_account_info_with_pubkey(acc).map(|mint| (mint.mint(), mint))
            })
            .collect()
    }
    async fn get_program_accounts_pod<T: Pod + Send>(
        &self,
        program_id: &Pubkey,
        filters: Option<Vec<AccountFilter>>,
    ) -> anyhow::Result<impl Iterator<Item = (Pubkey, T)>> {
        self.get_program_accounts_with(program_id, filters, |acc| {
            bytemuck::try_from_bytes(acc.data.as_slice()).ok().copied()
        })
        .await
    }
    async fn get_balances(
        &self,
        owner: &Pubkey,
        mints: &[TokenMint],
    ) -> anyhow::Result<HashMap<Pubkey, u64>> {
        let atas = mints
            .iter()
            .map(|x| x.get_associated_token_account_address(owner))
            .collect::<Vec<_>>();
        let accounts = self.get_multiple_accounts_batch(&atas).await?;
        Ok(accounts
            .iter()
            .filter_map(|acc| unpack_mint_balance(&acc.data))
            .collect())
    }
    async fn get_all_balances(&self, owner: &Pubkey) -> anyhow::Result<HashMap<Pubkey, u64>> {
        self.get_program_accounts_with(
            &apl_token::id(),
            Some(vec![AccountFilter::DataContent {
                offset: 32,
                bytes: owner.serialize().to_vec(),
            }]),
            |acc| unpack_mint_balance(&acc.data),
        )
        .await
        .map(|iter| iter.map(|(_, mint_balance)| mint_balance).collect())
    }
}

#[async_trait::async_trait]
impl ArchAsyncRpcExt for AsyncArchRpcClient {
    async fn get_multiple_accounts_batch(
        &self,
        pubkeys: &[Pubkey],
    ) -> anyhow::Result<GetMultipleAccountsBatch> {
        const MAX_BATCH_SIZE: usize = 100;
        let mut accounts = Vec::with_capacity(pubkeys.len() / MAX_BATCH_SIZE);
        for chunk in pubkeys.chunks(MAX_BATCH_SIZE) {
            accounts.push(self.get_multiple_accounts(chunk.to_vec()).await?);
        }
        Ok(GetMultipleAccountsBatch { accounts })
    }
    async fn get_program_accounts_with<T: Send>(
        &self,
        program_id: &Pubkey,
        filters: Option<Vec<AccountFilter>>,
        mut filter_map: impl FnMut(AccountInfo) -> Option<T> + Send + Sync,
    ) -> anyhow::Result<impl Iterator<Item = (Pubkey, T)>> {
        Ok(self
            .get_program_accounts(program_id, filters)
            .await?
            .into_iter()
            .filter_map(move |acc| filter_map(acc.account).map(|data| (acc.pubkey, data))))
    }
    async fn get_pod_account<T: Pod>(&self, key: &Pubkey) -> anyhow::Result<T> {
        let acc = self
            .read_account_info(*key)
            .await
            .context("failed to read account info")?;
        bytemuck::try_from_bytes(&acc.data[..std::mem::size_of::<T>()])
            .ok()
            .copied()
            .context("failed to deserialize account data")
    }
}

fn unpack_mint_balance(data: &[u8]) -> Option<(Pubkey, u64)> {
    let balance = data
        .get(64..72)
        .and_then(|bytes| Some(u64::from_le_bytes(bytes.try_into().ok()?)));
    let mint = apl_token::state::Account::unpack_account_mint(&data);
    Some((mint.copied()?, balance?))
}
