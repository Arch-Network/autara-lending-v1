use std::{ops::Deref, sync::Arc};

use anyhow::Context;
use arch_sdk::{
    arch_program::{pubkey::Pubkey, sanitized::ArchMessage},
    AsyncArchRpcClient, RuntimeTransaction,
};
use dashmap::DashSet;
use jsonrpsee::{
    core::{RpcResult, SubscriptionResult},
    types::{
        error::{INTERNAL_ERROR_CODE, INVALID_REQUEST_CODE},
        ErrorObjectOwned,
    },
    ConnectionId, PendingSubscriptionSink, RpcModule, SubscriptionMessage,
};

use crate::{
    api::{api::AutaraServerApiServer, market::FullMarket, types::*},
    client::{
        blockhash_cache::BlockhashCache,
        client_without_signer::AutaraFullClientWithoutSigner,
        read::{AutaraReadClient, BorrowPositionInfo, SupplyPositionInfo, UserPositionItem},
        shared_autara_state::AutaraSharedState,
    },
    rpc_ext::ArchAsyncRpcExt,
    test::TokenMinter,
};

pub struct AutataServerContext {
    pub client: AutaraFullClientWithoutSigner<Arc<AutaraSharedState>>,
    /// Each minter paired with the per-token faucet amount (raw units) that
    /// `initialize` mints to a new user. Configured server-side (per-token in
    /// tokens.json or the FAUCET_MINT_AMOUNT default) — no longer hardcoded.
    pub minters: Vec<(TokenMinter, u64)>,
    active_market_streams: DashSet<ConnectionId>,
    active_user_position_streams: DashSet<ConnectionId>,
}

impl Deref for AutataServerContext {
    type Target = AutaraFullClientWithoutSigner<Arc<AutaraSharedState>>;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

#[jsonrpsee::core::async_trait]
impl AutaraServerApiServer for AutataServerContext {
    async fn create_account(&self, request: UserParams) -> RpcResult<TransactionToSignResponse> {
        let client = self.async_arch_client();
        let faucet_value = client
            .call_method_with_params_raw::<_>("create_account_with_faucet", request.user)
            .await
            .internal("Failed to call create_account_with_faucet")?
            .context("create_account_with_faucet returned no payload")
            .internal("Failed to create account with faucet")?;
        let runtime_tx: RuntimeTransaction =
            serde_json::from_value(faucet_value).internal("Failed to parse faucet transaction")?;
        let message_hash = runtime_tx.message.hash();
        Ok(TransactionToSignResponse {
            transaction_to_sign: message_hash,
            message: runtime_tx.message.serialize(),
            signatures: runtime_tx.signatures,
        })
    }

    async fn initialize(&self, request: UserParams) -> RpcResult<()> {
        let _ = self
            .async_arch_client()
            .request_airdrop(request.user)
            .await
            .internal("Failed to aidrop")?;
        for (minter, faucet_amount) in &self.minters {
            minter
                .mint_to(&request.user, *faucet_amount)
                .await
                .internal("Failed to mint tokens")?;
        }
        Ok(())
    }

    async fn get_all_market_ids(&self) -> RpcResult<GetAllMarketsResponse> {
        let market_ids: Vec<Pubkey> = self
            .read_client()
            .all_markets_maybe_stale()
            .map(|(id, _, _)| id)
            .collect();
        Ok(GetAllMarketsResponse { market_ids })
    }

    async fn get_market_by_id(&self, request: GetMarketRequest) -> RpcResult<Option<FullMarket>> {
        Ok(self
            .read_client()
            .get_market(&request.market_id)
            .map(|wrapper| FullMarket::new_from_market(request.market_id, wrapper.owned())))
    }

    async fn get_all_markets_streamed(
        &self,
        subscription_sink: PendingSubscriptionSink,
    ) -> SubscriptionResult {
        let conn_id = subscription_sink.connection_id();

        if !self.active_market_streams.insert(conn_id) {
            subscription_sink
                .reject(ErrorObjectOwned::owned(
                    INVALID_REQUEST_CODE,
                    "A market stream is already active on this connection",
                    None::<()>,
                ))
                .await;
            return Ok(());
        }

        let result = async {
            let sink = subscription_sink.accept().await?;
            let markets = self
                .read_client()
                .all_markets_maybe_stale()
                .map(|(id, market, _)| FullMarket::new_from_market(id, market.owned()));
            for market in markets {
                let msg = SubscriptionMessage::from_json(&market)?;
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
        .await;

        self.active_market_streams.remove(&conn_id);
        result
    }

    async fn get_all_user_positions_streamed(
        &self,
        subscription_sink: PendingSubscriptionSink,
    ) -> SubscriptionResult {
        let conn_id = subscription_sink.connection_id();

        if !self.active_user_position_streams.insert(conn_id) {
            subscription_sink
                .reject(ErrorObjectOwned::owned(
                    INVALID_REQUEST_CODE,
                    "A user position stream is already active on this connection",
                    None::<()>,
                ))
                .await;
            return Ok(());
        }

        let result = async {
            let sink = subscription_sink.accept().await?;
            let read = self.read_client();

            for (_, supply_position) in read.all_supply_position() {
                let market = read.get_market(supply_position.market());
                let owned_atoms = market
                    .map(|m| {
                        m.market()
                            .supply_position_info(&supply_position)
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let item = UserPositionItem::Supply(SupplyPositionInfo {
                    supply_position: *supply_position,
                    owned_atoms,
                });
                let msg = SubscriptionMessage::from_json(&item)?;
                if sink.send(msg).await.is_err() {
                    break;
                }
            }

            for (_, borrow_position) in read.all_borrow_position() {
                let health = read
                    .get_market(borrow_position.market())
                    .map(|m| {
                        m.borrow_position_health(&borrow_position)
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let item = UserPositionItem::Borrow(BorrowPositionInfo {
                    borrow_position: *borrow_position,
                    health,
                });
                let msg = SubscriptionMessage::from_json(&item)?;
                if sink.send(msg).await.is_err() {
                    break;
                }
            }

            Ok(())
        }
        .await;

        self.active_user_position_streams.remove(&conn_id);
        result
    }

    async fn get_user_positions(
        &self,
        request: GetUserPositionsRequest,
    ) -> RpcResult<GetUserPositionsResponse> {
        let positions = self.read_client().user_positions(&request.user);
        Ok(GetUserPositionsResponse { positions })
    }

    async fn build_autara_transaction(
        &self,
        request: AutaraTransactionRequest,
    ) -> RpcResult<TransactionToSignResponse> {
        request.build_tx_tracing_params().trace();

        let tx = match request {
            AutaraTransactionRequest::Supply(request) => {
                self.tx_builder(&request.user)
                    .supply(&request.market_id, request.amount)
                    .await
            }
            AutaraTransactionRequest::DepositCollateral(request) => {
                self.tx_builder(&request.user)
                    .deposit_collateral(&request.market_id, request.amount)
                    .await
            }
            AutaraTransactionRequest::Withdraw(request) => {
                self.tx_builder(&request.user)
                    .withdraw_supply(&request.market_id, request.amount)
                    .await
            }
            AutaraTransactionRequest::WithdrawCollateral(request) => {
                self.tx_builder(&request.user)
                    .withdraw_collateral(&request.market_id, request.amount)
                    .await
            }
            AutaraTransactionRequest::Borrow(request) => {
                self.tx_builder(&request.user)
                    .borrow(&request.market_id, request.amount)
                    .await
            }
            AutaraTransactionRequest::Repay(request) => {
                self.tx_builder(&request.user)
                    .repay(&request.market_id, request.amount)
                    .await
            }
            AutaraTransactionRequest::UpdateConfig(request) => {
                self.tx_builder(&request.user)
                    .update_config(&request.market_id, request.config)
                    .await
            }
            AutaraTransactionRequest::UpdateGlobalConfig(request) => {
                self.tx_builder(&request.user)
                    .update_global_config(request.config)
                    .await
            }
            AutaraTransactionRequest::LiquidatePosition(request) => {
                self.tx_builder(&request.user)
                    .liquidate(
                        &request.market,
                        &request.liquidatee_position,
                        Some(request.liquidate.max_borrowed_atoms_to_repay),
                        Some(request.liquidate.min_collateral_atoms_to_receive),
                        request.liquidate.ix_callback,
                    )
                    .await
            }
            AutaraTransactionRequest::RedeemCuratorFees(request) => {
                self.tx_builder(&request.user)
                    .redeem_curator_fees(&request.market_id)
                    .await
            }
            AutaraTransactionRequest::RedeemProtocolFees(request) => {
                self.tx_builder(&request.user)
                    .redeem_protocol_fees(&request.market_id)
                    .await
            }
            AutaraTransactionRequest::CreateMarket(request) => self
                .tx_builder(&request.payer)
                .create_market(
                    request.curator,
                    request.payer,
                    request.params,
                    request.supply_mint,
                    request.collateral_mint,
                )
                .await
                .map(|x| x.1),
            AutaraTransactionRequest::WithdrawAndRepay(request) => {
                self.tx_builder(&request.user)
                    .withdraw_repay(&request.market_id, request.params)
                    .await
            }
            AutaraTransactionRequest::BorrowAndDeposit(request) => {
                self.tx_builder(&request.user)
                    .borrow_deposit(&request.market_id, request.params)
                    .await
            }
        }
        .internal("Failed to build transaction")?;
        Ok(TransactionToSignResponse {
            transaction_to_sign: tx.message_hash,
            message: tx.message.serialize(),
            signatures: vec![],
        })
    }

    async fn send_transaction(
        &self,
        request: SendTransactionRequest,
    ) -> RpcResult<SendTransactionResponse> {
        let message =
            ArchMessage::deserialize(&request.message).internal("Failed to deserialize message")?;
        let events = self
            .tx_broadcast()
            .broadcast_transaction(RuntimeTransaction {
                version: 0,
                signatures: request.signatures.clone(),
                message,
            })
            .await
            .internal("Failed to broadcast transaction")?;
        Ok(SendTransactionResponse {
            signature: request.signatures[0].clone(),
            events,
        })
    }

    async fn get_user_balances(&self, request: UserParams) -> RpcResult<GetUserBalancesResponse> {
        let balances = self
            .async_arch_client()
            .get_all_balances(&request.user)
            .await
            .internal("Failed to get user balances")?;
        Ok(GetUserBalancesResponse {
            balances: balances
                .into_iter()
                .filter_map(|(mint, amount)| {
                    self.client
                        .read_client()
                        .get_token_decimals(&mint)
                        .map(|token_decimals| UserBalance {
                            mint,
                            amount,
                            ui_amount: (amount as f64 / 10f64.powi(token_decimals as i32))
                                .to_string(),
                        })
                })
                .collect(),
        })
    }
}

pub async fn build_autara_server(
    read_client: Arc<AutaraSharedState>,
    minters: Vec<(TokenMinter, u64)>,
    arch_client: AsyncArchRpcClient,
) -> anyhow::Result<RpcModule<AutataServerContext>> {
    let context = AutataServerContext {
        client: AutaraFullClientWithoutSigner::new(
            read_client,
            arch_client.clone(),
            BlockhashCache::new(arch_client, None).await?,
        ),
        minters,
        active_market_streams: DashSet::new(),
        active_user_position_streams: DashSet::new(),
    };
    Ok(context.into_rpc())
}

trait ErrorMapper<T> {
    fn internal(self, context: &str) -> RpcResult<T>;
}

impl<T, E: std::fmt::Display> ErrorMapper<T> for Result<T, E> {
    fn internal(self, context: &str) -> RpcResult<T> {
        self.map_err(|e| {
            ErrorObjectOwned::owned(
                INTERNAL_ERROR_CODE,
                format!("context: {context}, error: {e}"),
                None::<()>,
            )
        })
    }
}
