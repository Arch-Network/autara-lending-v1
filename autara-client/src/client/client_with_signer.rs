use arch_sdk::{
    arch_program::{
        bitcoin::{key::Keypair, Network},
        instruction::Instruction,
        pubkey::Pubkey,
    },
    AsyncArchRpcClient,
};
use autara_lib::{
    event::AutaraEvents,
    ixs::{
        BorrowDepositAplInstruction, CreateMarketInstruction, UpdateConfigInstruction,
        UpdateGlobalConfigInstruction, WithdrawRepayAplInstruction,
    },
    state::borrow_position::BorrowPositionHealth,
};

use crate::client::{
    read::AutaraReadClient,
    single_thread_client::AutaraReadClientImpl,
    tx_broadcast::{AutaraClientError, AutaraTxBroadcast},
    tx_builder::AutaraTransactionBuilder,
};

pub struct AutaraFullClientWithSigner<T: AutaraReadClient> {
    read_client: T,
    arch_client: AsyncArchRpcClient,
    network: Network,
    signer: Keypair,
    signer_pubkey: Pubkey,
}

impl AutaraFullClientWithSigner<AutaraReadClientImpl> {
    pub fn new_simple(
        arch_client: AsyncArchRpcClient,
        network: Network,
        autara_program_id: Pubkey,
        signer: Keypair,
    ) -> Self {
        let read_client = AutaraReadClientImpl::new(arch_client.clone(), autara_program_id);
        Self {
            read_client,
            network,
            arch_client,
            signer_pubkey: Pubkey::from_slice(&signer.x_only_public_key().0.serialize()),
            signer,
        }
    }

    pub async fn full_reload(&mut self) -> anyhow::Result<()> {
        self.read_client.reload().await?;
        Ok(())
    }

    pub async fn reload_authority_accounts_for_market(
        &mut self,
        market: &Pubkey,
    ) -> anyhow::Result<()> {
        self.read_client
            .reload_authority_accounts_for_market(market, &self.signer_pubkey)
            .await
    }
}

impl<T: AutaraReadClient> AutaraFullClientWithSigner<T> {
    pub fn new(
        read_client: T,
        arch_client: AsyncArchRpcClient,
        network: Network,
        signer: Keypair,
    ) -> Self {
        let signer_pubkey = Pubkey::from_slice(&signer.x_only_public_key().0.serialize());
        Self {
            read_client,
            arch_client,
            network,
            signer,
            signer_pubkey,
        }
    }

    pub fn with_signer<'a>(&'a self, signer: Keypair) -> AutaraFullClientWithSigner<&'a T> {
        AutaraFullClientWithSigner {
            read_client: &self.read_client,
            arch_client: self.arch_client.clone(),
            network: self.network,
            signer_pubkey: Pubkey::from_slice(&signer.x_only_public_key().0.serialize()),
            signer,
        }
    }

    pub fn rpc_client(&self) -> &AsyncArchRpcClient {
        &self.arch_client
    }

    pub fn read_client<'a>(&'a self) -> &'a T {
        &self.read_client
    }

    pub fn signer_pubkey(&self) -> &Pubkey {
        &self.signer_pubkey
    }

    pub fn get_supply_position(
        &self,
        market: &Pubkey,
    ) -> Option<autara_lib::state::supply_position::SupplyPosition> {
        self.read_client
            .get_supply_position(market, &self.signer_pubkey)
            .1
            .as_deref()
            .copied()
    }

    pub fn get_borrow_position_health(
        &self,
        market: &Pubkey,
    ) -> anyhow::Result<BorrowPositionHealth> {
        self.read_client
            .get_borrow_position_health(market, &self.signer_pubkey)
    }

    pub async fn create_market(
        &self,
        create_market: CreateMarketInstruction,
        supply_mint: Pubkey,
        collateral_mint: Pubkey,
    ) -> Result<Pubkey, AutaraClientError> {
        let (market, tx) = self
            .tx_builder()
            .create_market(
                self.signer_pubkey,
                self.signer_pubkey,
                create_market,
                supply_mint,
                collateral_mint,
            )
            .await?;
        self.tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(market)
    }

    pub async fn create_global_config(
        &self,
        admin: Pubkey,
        fee_receiver: Pubkey,
        protocol_fee_share_in_bps: u16,
    ) -> Result<Pubkey, AutaraClientError> {
        let (global_config_pda, tx) = self
            .tx_builder()
            .create_global_config(
                self.signer_pubkey,
                admin,
                fee_receiver,
                protocol_fee_share_in_bps,
            )
            .await?;
        if let Err(err) = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await
        {
            if let AutaraClientError::Other(error) = &err {
                if error.to_string().contains("already exists") {
                    return Ok(global_config_pda);
                }
            }
            return Err(err);
        };
        Ok(global_config_pda)
    }

    pub async fn supply(
        &self,
        market: &Pubkey,
        amount: u64,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().supply(market, amount).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn deposit_collateral(
        &self,
        market: &Pubkey,
        amount: u64,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().deposit_collateral(market, amount).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn withdraw_collateral(
        &self,
        market: &Pubkey,
        amount: Option<u64>,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self
            .tx_builder()
            .withdraw_collateral(market, amount)
            .await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn borrow(
        &self,
        market: &Pubkey,
        amount: u64,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().borrow(market, amount).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn repay(
        &self,
        market: &Pubkey,
        amount: Option<u64>,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().repay(market, amount).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn withdraw_supply(
        &self,
        market: &Pubkey,
        amount: Option<u64>,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().withdraw_supply(market, amount).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn update_config(
        &self,
        market: &Pubkey,
        config: UpdateConfigInstruction,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().update_config(market, config).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn liquidate(
        &self,
        market: &Pubkey,
        borrow_position: &Pubkey,
        max_borrowed_atoms_to_repay: Option<u64>,
        min_collateral_atoms_to_receive: Option<u64>,
        ix_callback: Option<Instruction>,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self
            .tx_builder()
            .liquidate(
                market,
                borrow_position,
                max_borrowed_atoms_to_repay,
                min_collateral_atoms_to_receive,
                ix_callback,
            )
            .await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn reedeem_curator_fees(
        &self,
        market: &Pubkey,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().redeem_curator_fees(market).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn reedeem_protocol_fees(
        &self,
        market: &Pubkey,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().redeem_protocol_fees(market).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn update_global_config(
        &self,
        update: UpdateGlobalConfigInstruction,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().update_global_config(update).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn borrow_deposit(
        &self,
        market_key: &Pubkey,
        ix: BorrowDepositAplInstruction,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().borrow_deposit(market_key, ix).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn withdraw_repay(
        &self,
        market_key: &Pubkey,
        ix: WithdrawRepayAplInstruction,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().withdraw_repay(market_key, ix).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn donate_supply(
        &self,
        market: &Pubkey,
        amount: u64,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().donate_supply(market, amount).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub async fn socialize_loss(
        &self,
        market: &Pubkey,
        position: &Pubkey,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let tx = self.tx_builder().socialize_loss(market, position).await?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(tx.sign(&[self.signer], self.network))
            .await?;
        Ok(events)
    }

    pub fn tx_builder(&self) -> AutaraTransactionBuilder<'_, T> {
        AutaraTransactionBuilder {
            arch_client: &self.arch_client,
            autara_read_client: &self.read_client,
            autara_program_id: *self.read_client.autara_program_id(),
            authority_key: self.signer_pubkey,
            blockhash_cache: None,
        }
    }

    fn tx_broadcast(&self) -> AutaraTxBroadcast<'_> {
        AutaraTxBroadcast {
            program_id: self.read_client.autara_program_id(),
            arch_client: &self.arch_client,
        }
    }
}

impl<'a, T: AutaraReadClient> AutaraFullClientWithSigner<&'a T> {
    pub fn read_client_ref(&self) -> &'a T {
        self.read_client
    }
}
