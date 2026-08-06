use anyhow::Context;
use arch_sdk::{
    arch_program::{
        bitcoin::{key::Keypair, Network},
        instruction::Instruction,
        pubkey::Pubkey,
        sanitized::ArchMessage,
    },
    sign_message_bip322, AsyncArchRpcClient, RuntimeTransaction, Signature,
};
use autara_lib::{
    ixs::{
        reedeem_curator_fees_ix, reedeem_protocol_fees_ix, BorrowDepositAplInstruction,
        CreateMarketInstruction, WithdrawRepayAplInstruction,
    },
    token::create_ata_ix,
};
use autara_lib::{
    ixs::{UpdateConfigInstruction, UpdateGlobalConfigInstruction},
    token::get_associated_token_address,
};

use crate::client::{blockhash_cache::BlockhashCache, read::AutaraReadClient};

pub struct AutaraTransactionBuilder<'a, T: AutaraReadClient> {
    pub arch_client: &'a AsyncArchRpcClient,
    pub autara_read_client: &'a T,
    pub autara_program_id: Pubkey,
    pub authority_key: Pubkey,
    pub blockhash_cache: Option<&'a BlockhashCache>,
}

impl<'a, T: AutaraReadClient> AutaraTransactionBuilder<'a, T> {
    pub async fn create_market(
        &self,
        curator: Pubkey,
        payer: Pubkey,
        create_market: CreateMarketInstruction,
        supply_mint: Pubkey,
        collateral_mint: Pubkey,
    ) -> anyhow::Result<(Pubkey, TransactionToSign)> {
        let (market, ix) = autara_lib::ixs::create_market_ix(
            create_market,
            supply_mint,
            collateral_mint,
            self.autara_program_id,
            curator,
            payer,
        );
        Ok((
            market,
            self.build_transaction_digest_hash_to_sign(vec![ix]).await?,
        ))
    }

    pub async fn create_global_config(
        &self,
        payer: Pubkey,
        admin: Pubkey,
        fee_receiver: Pubkey,
        protocol_fee_share_in_bps: u16,
    ) -> anyhow::Result<(Pubkey, TransactionToSign)> {
        let (global_config_pda, ix) = autara_lib::ixs::create_global_config_ix(
            self.autara_program_id,
            payer,
            admin,
            fee_receiver,
            protocol_fee_share_in_bps,
        );
        Ok((
            global_config_pda,
            self.build_transaction_digest_hash_to_sign(vec![ix]).await?,
        ))
    }

    pub async fn supply(
        &self,
        market_key: &Pubkey,
        atoms: u64,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;

        let (supply_pda, supply_position) = self
            .autara_read_client
            .get_supply_position(market_key, &self.authority_key);
        let mut ixs = Vec::new();

        if supply_position.is_none() {
            let (_, ix) = autara_lib::ixs::create_supply_position_ix(
                self.autara_program_id,
                *market_key,
                self.authority_key,
                self.authority_key,
            );
            ixs.push(ix);
        }
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let supply_ix = autara_lib::ixs::supply_apl_ix(
            self.autara_program_id,
            *market_key,
            supply_pda,
            self.authority_key,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            atoms,
        );
        ixs.push(supply_ix);

        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn deposit_collateral(
        &self,
        market_key: &Pubkey,
        atoms: u64,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;

        let (borrow_pda, borrow_position) = self
            .autara_read_client
            .get_borrow_position(market_key, &self.authority_key);
        let mut ixs = Vec::new();

        if borrow_position.is_none() {
            let (_, ix) = autara_lib::ixs::create_borrow_position_ix(
                self.autara_program_id,
                *market_key,
                self.authority_key,
                self.authority_key,
            );
            ixs.push(ix);
        }

        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let collateral_ix = autara_lib::ixs::deposit_apl_collateral_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            borrow_pda,
            market
                .market()
                .collateral_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().collateral_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            atoms,
        );
        ixs.push(collateral_ix);

        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn borrow(
        &self,
        market_key: &Pubkey,
        atoms: u64,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;

        let (borrow_pda, _) = self
            .autara_read_client
            .get_borrow_position(market_key, &self.authority_key);
        let mut ixs = Vec::new();

        if let Some(ix) = self
            .maybe_create_ata(
                &self.authority_key,
                &market.market().supply_token_info().mint,
            )
            .await?
        {
            ixs.push(ix);
        }

        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let borrow_ix = autara_lib::ixs::borrow_apl_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            borrow_pda,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            atoms,
        );
        ixs.push(borrow_ix);

        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn repay(
        &self,
        market_key: &Pubkey,
        atoms: Option<u64>,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;

        let (borrow_pda, _) = self
            .autara_read_client
            .get_borrow_position(market_key, &self.authority_key);

        let mut ixs = Vec::new();
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let repay_ix = autara_lib::ixs::repay_apl_ix(
            self.autara_program_id,
            *market_key,
            borrow_pda,
            self.authority_key,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            atoms.unwrap_or(0),
            atoms.is_none(),
        );
        ixs.push(repay_ix);

        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn withdraw_supply(
        &self,
        market_key: &Pubkey,
        atoms: Option<u64>,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let (supply_pda, _) = self
            .autara_read_client
            .get_supply_position(market_key, &self.authority_key);
        let mut ixs = Vec::new();
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let withdraw_ix = autara_lib::ixs::withdraw_supply_ix(
            self.autara_program_id,
            *market_key,
            supply_pda,
            self.authority_key,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            atoms.unwrap_or(0),
            atoms.is_none(),
        );
        ixs.push(withdraw_ix);

        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn withdraw_collateral(
        &self,
        market_key: &Pubkey,
        atoms: Option<u64>,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let (borrow_pda, _) = self
            .autara_read_client
            .get_borrow_position(market_key, &self.authority_key);
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let mut ixs = Vec::new();
        let withdraw_ix = autara_lib::ixs::withdraw_apl_collateral_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            borrow_pda,
            market
                .market()
                .collateral_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().collateral_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            atoms.unwrap_or(0),
            atoms.is_none(),
        );
        ixs.push(withdraw_ix);

        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn update_config(
        &self,
        market_key: &Pubkey,
        config: UpdateConfigInstruction,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let oracle_keys = market.market().get_oracle_keys();
        let update_ix = autara_lib::ixs::update_config_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            config,
            oracle_keys.0,
            oracle_keys.1,
        );
        self.build_transaction_digest_hash_to_sign(vec![update_ix])
            .await
    }

    pub async fn liquidate(
        &self,
        market_key: &Pubkey,
        borrow_position_key: &Pubkey,
        max_borrowed_atoms_to_repay: Option<u64>,
        min_collateral_atoms_to_receive: Option<u64>,
        ix_callback: Option<Instruction>,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let mut ixs = vec![];
        if let Some(ix) = self
            .maybe_create_ata(&self.authority_key, market.market().supply_vault().mint())
            .await?
        {
            ixs.push(ix);
        }
        if let Some(ix) = self
            .maybe_create_ata(
                &self.authority_key,
                market.market().collateral_vault().mint(),
            )
            .await?
        {
            ixs.push(ix);
        }
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let liquidate_ix = autara_lib::ixs::liquidate_ix(
            self.autara_program_id,
            *market_key,
            *borrow_position_key,
            self.authority_key,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            market
                .market()
                .collateral_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            *market.market().collateral_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            max_borrowed_atoms_to_repay.unwrap_or(u64::MAX),
            min_collateral_atoms_to_receive.unwrap_or(0),
            ix_callback,
        );
        ixs.push(liquidate_ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn redeem_curator_fees(
        &self,
        market_key: &Pubkey,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(&market_key)
            .context("market not found")?;
        let mut ixs = vec![];
        if let Some(ix) = self
            .maybe_create_ata(&self.authority_key, market.market().supply_vault().mint())
            .await?
        {
            ixs.push(ix);
        }
        let ix = reedeem_curator_fees_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
        );
        ixs.push(ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn redeem_protocol_fees(
        &self,
        market_key: &Pubkey,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(&market_key)
            .context("market not found")?;
        let fee_receiver = self.authority_key;
        let mut ixs = vec![];
        if let Some(ix) = self
            .maybe_create_ata(&fee_receiver, &market.market().supply_token_info().mint)
            .await?
        {
            ixs.push(ix);
        }
        let ix = reedeem_protocol_fees_ix(
            self.autara_program_id,
            *market_key,
            fee_receiver,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&fee_receiver),
            *market.market().supply_vault().vault(),
        );
        ixs.push(ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn update_global_config(
        &self,
        update: UpdateGlobalConfigInstruction,
    ) -> anyhow::Result<TransactionToSign> {
        let ix = autara_lib::ixs::update_global_config_ix(
            self.autara_program_id,
            self.authority_key,
            update,
        );
        Ok(self.build_transaction_digest_hash_to_sign(vec![ix]).await?)
    }

    pub async fn borrow_deposit(
        &self,
        market_key: &Pubkey,
        ix: BorrowDepositAplInstruction,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let (borrow_pda, borrow_position) = self
            .autara_read_client
            .get_borrow_position(market_key, &self.authority_key);
        let mut ixs = Vec::new();
        if borrow_position.is_none() {
            let (_, ix) = autara_lib::ixs::create_borrow_position_ix(
                self.autara_program_id,
                *market_key,
                self.authority_key,
                self.authority_key,
            );
            ixs.push(ix);
        }
        drop(borrow_position);
        if let Some(ix) = self
            .maybe_create_ata(&self.authority_key, market.market().supply_vault().mint())
            .await?
        {
            ixs.push(ix);
        }
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let borrow_deposit_ix = autara_lib::ixs::borrow_deposit_apl_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            borrow_pda,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            market
                .market()
                .collateral_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().collateral_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            ix,
        );
        ixs.push(borrow_deposit_ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn withdraw_repay(
        &self,
        market_key: &Pubkey,
        ix: WithdrawRepayAplInstruction,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let (borrow_pda, _) = self
            .autara_read_client
            .get_borrow_position(market_key, &self.authority_key);
        let mut ixs = Vec::new();
        if let Some(ix) = self
            .maybe_create_ata(&self.authority_key, market.market().supply_vault().mint())
            .await?
        {
            ixs.push(ix);
        }
        if let Some(ix) = self
            .maybe_create_ata(
                &self.authority_key,
                market.market().collateral_vault().mint(),
            )
            .await?
        {
            ixs.push(ix);
        }
        let (supply_oracle_id, collateral_oracle_id) = market.market().get_oracle_keys();
        let withdraw_repay_ix = autara_lib::ixs::withdraw_repay_apl_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            borrow_pda,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            market
                .market()
                .collateral_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().collateral_vault().vault(),
            supply_oracle_id,
            collateral_oracle_id,
            ix,
        );
        ixs.push(withdraw_repay_ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn socialize_loss(
        &self,
        market_key: &Pubkey,
        position: &Pubkey,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;
        let oracles = market.market().get_oracle_keys();
        let mut ixs = Vec::new();
        if let Some(ix) = self
            .maybe_create_ata(
                &self.authority_key,
                market.market().collateral_vault().mint(),
            )
            .await?
        {
            ixs.push(ix);
        }
        let ix = autara_lib::ixs::socialize_loss_ix(
            self.autara_program_id,
            *market_key,
            *position,
            self.authority_key,
            market
                .market()
                .collateral_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().collateral_vault().vault(),
            oracles.0,
            oracles.1,
        );
        ixs.push(ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    pub async fn donate_supply(
        &self,
        market_key: &Pubkey,
        atoms: u64,
    ) -> anyhow::Result<TransactionToSign> {
        let market = self
            .autara_read_client
            .get_market(market_key)
            .context("market not found")?;

        let mut ixs = Vec::new();
        let supply_ix = autara_lib::ixs::donate_supply_ix(
            self.autara_program_id,
            *market_key,
            self.authority_key,
            market
                .market()
                .supply_token_info()
                .get_associated_token_address(&self.authority_key),
            *market.market().supply_vault().vault(),
            atoms,
        );
        ixs.push(supply_ix);
        self.build_transaction_digest_hash_to_sign(ixs).await
    }

    async fn build_transaction_digest_hash_to_sign(
        &self,
        ixs: Vec<Instruction>,
    ) -> anyhow::Result<TransactionToSign> {
        let message = ArchMessage::new(
            &ixs,
            Some(self.authority_key),
            if let Some(cache) = self.blockhash_cache {
                cache.get_blockhash()
            } else {
                self.arch_client.get_best_block_hash().await?
            },
        );
        Ok(TransactionToSign {
            message_hash: message.hash(),
            message,
            instructions: ixs,
        })
    }

    async fn maybe_create_ata(
        &self,
        owner: &Pubkey,
        mint: &Pubkey,
    ) -> anyhow::Result<Option<Instruction>> {
        let ata = get_associated_token_address(owner, mint);
        let ix = || Some(create_ata_ix(owner, Some(&ata), owner, mint));
        match self.arch_client.read_account_info(ata).await {
            Ok(_) => Ok(None),
            Err(arch_sdk::ArchError::NotFound(_)) => Ok(ix()),
            Err(e) => {
                if let arch_sdk::ArchError::RpcRequestFailed(msg) = &e {
                    if msg.contains("account is not in database") {
                        return Ok(ix());
                    }
                }
                return Err(e.into());
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransactionToSign {
    pub instructions: Vec<Instruction>,
    pub message: ArchMessage,
    pub message_hash: Vec<u8>,
}

impl TransactionToSign {
    pub fn sign(&self, signers: &[Keypair], network: Network) -> RuntimeTransaction {
        let signatures = self
            .message
            .account_keys
            .iter()
            .take(self.message.header.num_required_signatures as usize)
            .map(|key| {
                let sign = sign_message_bip322(
                    signers.iter().find(|signer| {
                        signer.x_only_public_key().0.serialize() == key.serialize()
                    })?,
                    &self.message_hash,
                    network,
                );
                Some(Signature(sign))
            })
            .collect::<Option<Vec<Signature>>>()
            .unwrap();
        RuntimeTransaction {
            version: 0,
            signatures,
            message: self.message.clone(),
        }
    }
}
