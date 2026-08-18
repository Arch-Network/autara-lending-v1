use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Context;
use arch_sdk::{arch_program::pubkey::Pubkey, AccountInfoWithPubkey, AsyncArchRpcClient};
use autara_lib::{
    pda::{find_borrow_position_pda, find_global_config_pda, find_supply_position_pda},
    state::{
        borrow_position::BorrowPosition, global_config::GlobalConfig, market::Market,
        market_wrapper::MarketWrapper, supply_position::SupplyPosition,
    },
};
use dashmap::DashMap;

use crate::{
    client::{read::AutaraReadClient, single_thread_client::get_unix_timestamp},
    filter::{borrow_position_filter, market_filter, supply_position_filter},
    rpc_ext::ArchAsyncRpcExt,
};

pub struct AutaraSharedState {
    arch_client: AsyncArchRpcClient,
    autara_program_id: Pubkey,
    market_map: DashMap<Pubkey, Market>,
    supply_position_map: DashMap<Pubkey, SupplyPosition>,
    borrow_position_map: DashMap<Pubkey, BorrowPosition>,
    oracle_map: DashMap<Pubkey, AccountInfoWithPubkey>,
    mint_decimals: DashMap<Pubkey, u8>,
    global_config: RwLock<GlobalConfig>,
}

impl AutaraSharedState {
    pub fn new(arch_client: AsyncArchRpcClient, autara_program_id: Pubkey) -> Self {
        Self {
            arch_client,
            autara_program_id,
            market_map: DashMap::new(),
            supply_position_map: DashMap::new(),
            borrow_position_map: DashMap::new(),
            oracle_map: DashMap::new(),
            mint_decimals: DashMap::new(),
            global_config: RwLock::new(GlobalConfig::default()),
        }
    }

    pub fn get_token_decimals(&self, mint: &Pubkey) -> Option<u8> {
        self.mint_decimals.get(mint).map(|r| *r.value())
    }

    pub fn spawn(self) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let state = Arc::new(self);
        let handle = tokio::spawn({
            let state = Arc::clone(&state);
            async move {
                loop {
                    if let Err(e) = tokio::time::timeout(Duration::from_secs(10), state.reload())
                        .await
                        .context("Timeout while reloading Autara state")
                        .and_then(|result| result)
                    {
                        tracing::error!("Failed to reload {:?}", e);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            }
        });
        (state, handle)
    }

    pub async fn reload(&self) -> anyhow::Result<()> {
        let global_pda = find_global_config_pda(&self.autara_program_id).0;
        let (supply, borrow, markets, global_config) = tokio::try_join!(
            self.arch_client.get_program_accounts_pod::<SupplyPosition>(
                &self.autara_program_id,
                Some(supply_position_filter()),
            ),
            self.arch_client.get_program_accounts_pod::<BorrowPosition>(
                &self.autara_program_id,
                Some(borrow_position_filter()),
            ),
            self.arch_client
                .get_program_accounts_pod::<Market>(&self.autara_program_id, Some(market_filter())),
            self.arch_client.get_pod_account(&global_pda)
        )?;
        for (key, account) in supply {
            self.supply_position_map.insert(key, account);
        }
        for (key, account) in borrow {
            self.borrow_position_map.insert(key, account);
        }
        *self.global_config.write().unwrap() = global_config;

        // Collect oracle keys from markets and fetch them in batch
        let markets: Vec<(Pubkey, Market)> = markets.collect();
        let oracle_keys: Vec<Pubkey> = markets
            .iter()
            .flat_map(|(_, m)| {
                let keys = m.get_oracle_keys();
                [keys.0, keys.1]
            })
            .collect();
        let accs = self
            .arch_client
            .get_multiple_accounts_batch(&oracle_keys)
            .await
            .context("failed to fetch oracle accounts")?;
        for acc in accs.into_iter() {
            self.oracle_map.insert(acc.key, acc);
        }

        let ts = get_unix_timestamp();
        for (key, market) in markets {
            if let Err(e) = self.process_single_market(key, market, ts) {
                tracing::error!("Failed to process market {}: {:?}", key, e);
            }
        }
        Ok(())
    }

    fn process_single_market(
        &self,
        key: Pubkey,
        mut market: Market,
        ts: i64,
    ) -> anyhow::Result<()> {
        for token_info in [market.supply_token_info(), market.collateral_token_info()] {
            self.mint_decimals
                .entry(token_info.mint)
                .or_insert(token_info.decimals);
        }
        let (supply_oracle_id, collateral_oracle_id) = market.get_oracle_keys();
        let supply_oracle = self
            .oracle_map
            .get(&supply_oracle_id)
            .context("supply oracle not found")?;
        let collateral_oracle = self
            .oracle_map
            .get(&collateral_oracle_id)
            .context("collateral oracle not found")?;
        market
            .wrapper_mut(
                supply_oracle.value().into(),
                collateral_oracle.value().into(),
                ts,
            )?
            .sync_clock(ts)?;
        self.market_map.insert(key, market);
        Ok(())
    }

    fn load_market_wrapper<T: std::ops::Deref<Target = Market>>(
        &self,
        market: T,
    ) -> Option<MarketWrapper<T>> {
        let (supply_oracle_id, collateral_oracle_id) = market.get_oracle_keys();
        let supply_oracle = self.oracle_map.get(&supply_oracle_id)?;
        let collateral_oracle = self.oracle_map.get(&collateral_oracle_id)?;
        MarketWrapper::try_new(
            market,
            supply_oracle.value().into(),
            collateral_oracle.value().into(),
            get_unix_timestamp(),
        )
        .ok()
    }

    fn load_market_wrapper_maybe_stale<T: std::ops::Deref<Target = Market>>(
        &self,
        market: T,
    ) -> Option<(MarketWrapper<T>, bool)> {
        let (supply_oracle_id, collateral_oracle_id) = market.get_oracle_keys();
        let supply_oracle = self.oracle_map.get(&supply_oracle_id)?;
        let collateral_oracle = self.oracle_map.get(&collateral_oracle_id)?;
        MarketWrapper::try_new_or_unchecked(
            market,
            supply_oracle.value().into(),
            collateral_oracle.value().into(),
            get_unix_timestamp(),
        )
        .ok()
    }
}

impl AutaraReadClient for AutaraSharedState {
    fn autara_program_id(&self) -> &Pubkey {
        &self.autara_program_id
    }

    fn all_markets(
        &self,
    ) -> impl Iterator<Item = (Pubkey, MarketWrapper<impl std::ops::Deref<Target = Market>>)> {
        self.market_map.iter().filter_map(|r| {
            let key = *r.key();
            self.load_market_wrapper(r).map(|m| (key, m))
        })
    }

    fn all_markets_maybe_stale(
        &self,
    ) -> impl Iterator<
        Item = (
            Pubkey,
            MarketWrapper<impl std::ops::Deref<Target = Market>>,
            bool,
        ),
    > {
        self.market_map.iter().filter_map(|r| {
            let key = *r.key();
            self.load_market_wrapper_maybe_stale(r)
                .map(|(m, stale)| (key, m, stale))
        })
    }

    fn all_borrow_position(
        &self,
    ) -> impl Iterator<Item = (Pubkey, impl std::ops::Deref<Target = BorrowPosition>)> {
        self.borrow_position_map.iter().map(|r| (*r.key(), r))
    }

    fn all_supply_position(
        &self,
    ) -> impl Iterator<Item = (Pubkey, impl std::ops::Deref<Target = SupplyPosition>)> {
        self.supply_position_map.iter().map(|r| (*r.key(), r))
    }

    fn get_market(
        &self,
        market: &Pubkey,
    ) -> Option<MarketWrapper<impl std::ops::Deref<Target = Market>>> {
        self.market_map
            .get(market)
            .and_then(|market| self.load_market_wrapper(market))
    }

    fn get_borrow_position(
        &self,
        market: &Pubkey,
        authority: &Pubkey,
    ) -> (
        Pubkey,
        Option<impl std::ops::Deref<Target = BorrowPosition>>,
    ) {
        let borrow_position =
            find_borrow_position_pda(&self.autara_program_id, market, authority).0;
        (
            borrow_position,
            self.borrow_position_map.get(&borrow_position),
        )
    }

    fn get_supply_position(
        &self,
        market: &Pubkey,
        authority: &Pubkey,
    ) -> (
        Pubkey,
        Option<impl std::ops::Deref<Target = SupplyPosition>>,
    ) {
        let supply_position =
            find_supply_position_pda(&self.autara_program_id, market, authority).0;
        (
            supply_position,
            self.supply_position_map.get(&supply_position),
        )
    }

    fn get_global_config(&self) -> Option<impl std::ops::Deref<Target = GlobalConfig>> {
        self.global_config.read().ok()
    }
}
