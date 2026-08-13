use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
};

use anyhow::Context;
use arch_sdk::{
    arch_program::pubkey::Pubkey, AccountFilter, AccountInfoWithPubkey, AsyncArchRpcClient,
};
use autara_lib::{
    pda::{find_borrow_position_pda, find_global_config_pda, find_supply_position_pda},
    state::{
        borrow_position::{BorrowPosition, BorrowPositionHealth},
        global_config::GlobalConfig,
        market::Market,
        market_wrapper::MarketWrapper,
        supply_position::SupplyPosition,
    },
};
use bytemuck::Pod;

use crate::{
    client::read::AutaraReadClient,
    filter::{borrow_position_filter, market_filter, supply_position_filter},
    rpc_ext::ArchAsyncRpcExt,
};

#[derive(Clone)]
pub struct AutaraReadClientImpl {
    arch_client: AsyncArchRpcClient,
    autara_program_id: Pubkey,
    market_map: HashMap<Pubkey, Market>,
    supply_position_map: HashMap<Pubkey, SupplyPosition>,
    borrow_position_map: HashMap<Pubkey, BorrowPosition>,
    oracle_map: HashMap<Pubkey, AccountInfoWithPubkey>,
    global_config_map: GlobalConfig,
}

impl AutaraReadClientImpl {
    pub fn new(arch_client: AsyncArchRpcClient, autara_program_id: Pubkey) -> Self {
        Self {
            arch_client,
            autara_program_id,
            market_map: HashMap::new(),
            supply_position_map: HashMap::new(),
            borrow_position_map: HashMap::new(),
            oracle_map: HashMap::new(),
            global_config_map: GlobalConfig::default(),
        }
    }

    pub fn async_arch_client(&self) -> &AsyncArchRpcClient {
        &self.arch_client
    }

    pub async fn reload(&mut self) -> anyhow::Result<()> {
        let global_config_key = find_global_config_pda(&self.autara_program_id).0;

        let (markets, supply, borrow, global) = tokio::try_join!(
            self.load_program_accounts_pod(&self.autara_program_id, Some(market_filter())),
            self.load_program_accounts_pod(&self.autara_program_id, Some(supply_position_filter())),
            self.load_program_accounts_pod(&self.autara_program_id, Some(borrow_position_filter())),
            self.get_pod_account(&global_config_key),
        )?;
        self.market_map = markets;
        self.supply_position_map = supply;
        self.borrow_position_map = borrow;
        self.global_config_map = global;
        let ts = get_unix_timestamp();
        let oracles = self
            .market_map
            .values()
            .flat_map(|m| {
                let keys = m.get_oracle_keys();
                [keys.0, keys.1]
            })
            .collect::<Vec<_>>();
        let accs = self
            .arch_client
            .get_multiple_accounts_batch(&oracles)
            .await
            .context("failed to fetch oracle accounts")?;
        for acc in accs.into_iter() {
            self.oracle_map.insert(acc.key, acc);
        }
        for market in self.market_map.values_mut() {
            let _ = Self::inner_reload_market(&self.oracle_map, market, ts);
        }
        Ok(())
    }

    pub async fn reload_authority_accounts_for_market(
        &mut self,
        market_key: &Pubkey,
        authority: &Pubkey,
    ) -> anyhow::Result<()> {
        self.reload_market(market_key).await?;
        let (supply_position_key, _) = self.get_supply_position(market_key, authority);
        let _ = self.reload_supply_position(&supply_position_key).await;
        let (borrow_position_key, _) = self.get_borrow_position(market_key, authority);
        let _ = self.reload_borrow_position(&borrow_position_key).await;
        Ok(())
    }

    pub async fn reload_market(&mut self, market_key: &Pubkey) -> anyhow::Result<()> {
        let mut market: Market = self
            .get_pod_account(market_key)
            .await
            .context("failed to deserialize lending market account")?;
        let (supply_oracle_id, collateral_oracle_id) = market.get_oracle_keys();
        let accs = self
            .arch_client
            .get_multiple_accounts_batch(&[supply_oracle_id, collateral_oracle_id])
            .await?;
        accs.into_iter().for_each(|acc| {
            self.oracle_map.insert(acc.key, acc);
        });
        let ts = get_unix_timestamp();
        Self::inner_reload_market(&self.oracle_map, &mut market, ts)?;
        self.market_map.insert(*market_key, market);
        Ok(())
    }

    pub async fn reload_supply_position(
        &mut self,
        supply_position_key: &Pubkey,
    ) -> anyhow::Result<()> {
        let supply_position: SupplyPosition = self
            .get_pod_account(supply_position_key)
            .await
            .context("failed to deserialize lending position account")?;
        self.supply_position_map
            .insert(*supply_position_key, supply_position);
        Ok(())
    }

    pub async fn reload_borrow_position(
        &mut self,
        borrow_position_key: &Pubkey,
    ) -> anyhow::Result<()> {
        let borrow_position: BorrowPosition = self
            .get_pod_account(borrow_position_key)
            .await
            .context("failed to deserialize borrow position account")?;
        self.borrow_position_map
            .insert(*borrow_position_key, borrow_position);
        Ok(())
    }

    pub async fn reload_global_config(&mut self) -> anyhow::Result<()> {
        self.global_config_map = self
            .get_pod_account(&find_global_config_pda(&self.autara_program_id).0)
            .await
            .unwrap_or_default();
        Ok(())
    }

    pub async fn load_markets(&mut self) -> anyhow::Result<()> {
        self.market_map = self
            .load_program_accounts_pod(&self.autara_program_id, Some(market_filter()))
            .await
            .context("failed to load autara markets")?;
        Ok(())
    }

    pub async fn load_supply_positions(&mut self) -> anyhow::Result<()> {
        self.supply_position_map = self
            .load_program_accounts_pod(&self.autara_program_id, Some(supply_position_filter()))
            .await
            .context("failed to load autara supply positions")?;
        Ok(())
    }

    pub async fn load_borrow_positions(&mut self) -> anyhow::Result<()> {
        self.borrow_position_map = self
            .load_program_accounts_pod(&self.autara_program_id, Some(borrow_position_filter()))
            .await
            .context("failed to load autara borrow positions")?;
        Ok(())
    }

    pub fn all_tokens(&self) -> HashSet<Pubkey> {
        self.market_map
            .iter()
            .flat_map(|(_, m)| [m.collateral_token_info().mint, m.supply_token_info().mint])
            .collect()
    }

    pub fn get_borrow_position_health(
        &self,
        market: &Pubkey,
        authority: &Pubkey,
    ) -> anyhow::Result<BorrowPositionHealth> {
        let borrow_position = self
            .get_borrow_position(&market, authority)
            .1
            .context("borrow position not found")?;
        let market_w = self.get_market(&market).context("market not found")?;
        Ok(market_w.borrow_position_health(&borrow_position)?)
    }

    async fn get_pod_account<T: Pod>(&self, key: &Pubkey) -> anyhow::Result<T> {
        self.arch_client.get_pod_account(key).await
    }

    async fn load_program_accounts_pod<T: Pod + Send>(
        &self,
        program_id: &Pubkey,
        filters: Option<Vec<AccountFilter>>,
    ) -> anyhow::Result<HashMap<Pubkey, T>> {
        let accounts = self
            .arch_client
            .get_program_accounts_pod::<T>(program_id, filters)
            .await
            .context("failed to load program accounts")?;
        Ok(accounts.collect())
    }

    fn inner_reload_market(
        oracle_map: &HashMap<Pubkey, AccountInfoWithPubkey>,
        market: &mut Market,
        unix_timestamp: i64,
    ) -> anyhow::Result<()> {
        let (supply_oracle_id, collateral_oracle_id) = market.get_oracle_keys();
        let supply_oracle = oracle_map
            .get(&supply_oracle_id)
            .context("supply oracle not found")?;
        let collateral_oracle = oracle_map
            .get(&collateral_oracle_id)
            .context("collateral oracle not found")?;
        market
            .wrapper_mut(
                supply_oracle.into(),
                collateral_oracle.into(),
                unix_timestamp,
            )?
            .sync_clock(unix_timestamp)?;
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
            supply_oracle.into(),
            collateral_oracle.into(),
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
            supply_oracle.into(),
            collateral_oracle.into(),
            get_unix_timestamp(),
        )
        .ok()
    }
}

impl AutaraReadClient for AutaraReadClientImpl {
    fn autara_program_id(&self) -> &Pubkey {
        &self.autara_program_id
    }

    fn get_market(&self, market: &Pubkey) -> Option<MarketWrapper<impl Deref<Target = Market>>> {
        self.market_map
            .get(market)
            .and_then(|market| self.load_market_wrapper(market))
    }

    fn all_markets(
        &self,
    ) -> impl Iterator<Item = (Pubkey, MarketWrapper<impl Deref<Target = Market>>)> {
        self.market_map
            .iter()
            .filter_map(|(k, v)| self.load_market_wrapper(v).map(|m| (*k, m)))
    }

    fn all_markets_maybe_stale(
        &self,
    ) -> impl Iterator<Item = (Pubkey, MarketWrapper<impl Deref<Target = Market>>, bool)> {
        self.market_map.iter().filter_map(|(k, v)| {
            self.load_market_wrapper_maybe_stale(v)
                .map(|(m, stale)| (*k, m, stale))
        })
    }

    fn all_borrow_position(
        &self,
    ) -> impl Iterator<Item = (Pubkey, impl Deref<Target = BorrowPosition>)> {
        self.borrow_position_map.iter().map(|r| (*r.0, r.1))
    }

    fn all_supply_position(
        &self,
    ) -> impl Iterator<Item = (Pubkey, impl Deref<Target = SupplyPosition>)> {
        self.supply_position_map.iter().map(|r| (*r.0, r.1))
    }

    fn get_borrow_position(
        &self,
        market: &Pubkey,
        authority: &Pubkey,
    ) -> (Pubkey, Option<impl Deref<Target = BorrowPosition>>) {
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
    ) -> (Pubkey, Option<impl Deref<Target = SupplyPosition>>) {
        let supply_position =
            find_supply_position_pda(&self.autara_program_id, market, authority).0;
        (
            supply_position,
            self.supply_position_map.get(&supply_position),
        )
    }

    fn get_global_config(&self) -> Option<impl Deref<Target = GlobalConfig>> {
        Some(&self.global_config_map)
    }
}

pub fn get_unix_timestamp() -> i64 {
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap() as i64
}
