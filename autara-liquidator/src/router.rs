use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use anyhow::Result;
use arch_sdk::{ArchRpcClient, arch_program::pubkey::Pubkey};
use orca_whirlpools::{
    InitializedPool, PoolInfo, SwapQuote, SwapType, fetch_whirlpools_by_token_pair,
    swap_instructions_with_options,
};

/// 1% slippage tolerance
const SLIPPAGE_TOLERANCE_BPS: u16 = 100;

/// How often to re-discover pools
pub const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5 * 60);

fn sort_pair(a: Pubkey, b: Pubkey) -> (Pubkey, Pubkey) {
    if a < b { (a, b) } else { (b, a) }
}

type PoolCache = HashMap<(Pubkey, Pubkey), Vec<InitializedPool>>;

/// A swap router service that discovers whirlpool pools and finds the best
/// quote among available pools.
///
/// Pool discovery runs periodically via `maybe_refresh_pools()`. The cache is
/// behind an `RwLock` so quoting only needs `&self`.
pub struct SwapRouter {
    rpc: ArchRpcClient,
    pool_cache: RwLock<PoolCache>,
    last_discovery: RwLock<Option<Instant>>,
}

impl SwapRouter {
    pub fn new(rpc: ArchRpcClient) -> Self {
        Self {
            rpc,
            pool_cache: RwLock::new(HashMap::new()),
            last_discovery: RwLock::new(None),
        }
    }

    /// Register a token pair for discovery.
    pub async fn register_pair(&self, token_a: Pubkey, token_b: Pubkey) -> Result<()> {
        let key = sort_pair(token_a, token_b);
        if self.pool_cache.read().unwrap().contains_key(&key) {
            return Ok(());
        }
        let initialized = fetch_initialized_pools(&self.rpc, token_a, token_b).await?;
        tracing::debug!("register_pair: acquiring write lock (pool_cache)");
        self.pool_cache.write().unwrap().insert(key, initialized);
        tracing::debug!("register_pair: write lock released");
        Ok(())
    }

    /// Re-discover all registered pairs if enough time has elapsed.
    /// Call this from the main loop.
    pub async fn maybe_refresh_pools(&self) {
        let should_refresh = {
            let last = self.last_discovery.read().unwrap();
            last.map_or(true, |t| t.elapsed() >= DISCOVERY_INTERVAL)
        };

        if !should_refresh {
            return;
        }

        let pairs: Vec<(Pubkey, Pubkey)> =
            { self.pool_cache.read().unwrap().keys().copied().collect() };

        if pairs.is_empty() {
            tracing::debug!("maybe_refresh_pools: acquiring write lock (last_discovery, empty)");
            *self.last_discovery.write().unwrap() = Some(Instant::now());
            tracing::debug!("maybe_refresh_pools: write lock released (last_discovery, empty)");
            return;
        }

        tracing::info!("Refreshing {} pool pair(s)", pairs.len());

        for (token_a, token_b) in pairs {
            match fetch_initialized_pools(&self.rpc, token_a, token_b).await {
                Ok(pools) => {
                    let key = sort_pair(token_a, token_b);
                    tracing::debug!("maybe_refresh_pools: acquiring write lock (pool_cache)");
                    self.pool_cache.write().unwrap().insert(key, pools);
                    tracing::debug!("maybe_refresh_pools: write lock released (pool_cache)");
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to refresh pools for {:?} <-> {:?}: {:#}",
                        token_a,
                        token_b,
                        e
                    );
                }
            }
        }

        tracing::debug!("maybe_refresh_pools: acquiring write lock (last_discovery)");
        *self.last_discovery.write().unwrap() = Some(Instant::now());
        tracing::debug!("maybe_refresh_pools: write lock released (last_discovery)");
    }

    /// Get the best ExactIn swap quote across all cached pools for a pair.
    /// If no pools are cached for this pair, discovers them on the fly.
    pub async fn best_quote_exact_in(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount_in: u64,
        signer: Option<Pubkey>,
    ) -> Result<Option<(Pubkey, SwapQuote)>> {
        let key = sort_pair(input_mint, output_mint);

        tracing::debug!("best_quote_exact_in: acquiring read lock (has_pools check)");
        let has_pools = self.pool_cache.read().unwrap().contains_key(&key);
        tracing::debug!("best_quote_exact_in: has_pools={}", has_pools);

        if !has_pools {
            tracing::debug!("best_quote_exact_in: registering pair {:?} <-> {:?}", input_mint, output_mint);
            self.register_pair(input_mint, output_mint).await?;
            tracing::debug!("best_quote_exact_in: pair registered");
        }

        tracing::debug!("best_quote_exact_in: acquiring read lock (pool_addresses)");
        let pool_addresses: Vec<Pubkey> = self
            .pool_cache
            .read()
            .unwrap()
            .get(&key)
            .map(|pools| pools.iter().map(|p| p.address).collect())
            .unwrap_or_default();
        tracing::debug!("best_quote_exact_in: found {} pool(s)", pool_addresses.len());

        if pool_addresses.is_empty() {
            return Ok(None);
        }

        let mut best: Option<(Pubkey, SwapQuote, u64)> = None;

        for (i, pool_addr) in pool_addresses.iter().enumerate() {
            tracing::debug!("best_quote_exact_in: quoting pool {}/{} {:?}", i + 1, pool_addresses.len(), pool_addr);
            let quote_start = std::time::Instant::now();
            let result = swap_instructions_with_options(
                &self.rpc,
                *pool_addr,
                amount_in,
                input_mint,
                SwapType::ExactIn,
                Some(SLIPPAGE_TOLERANCE_BPS),
                signer,
                true,
            )
            .await;
            tracing::debug!("best_quote_exact_in: pool {:?} quote took {:?}", pool_addr, quote_start.elapsed());

            match result {
                Ok(swap_ix) => {
                    let est_out = match &swap_ix.quote {
                        SwapQuote::ExactIn(q) => q.token_est_out,
                        SwapQuote::ExactOut(_) => unreachable!(),
                    };

                    let is_better = best.as_ref().map_or(true, |(_, _, prev)| est_out > *prev);
                    if is_better {
                        best = Some((*pool_addr, swap_ix.quote, est_out));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        ?pool_addr,
                        "Swap quote failed (pool may have no liquidity): {}",
                        e
                    );
                }
            }
        }

        Ok(best.map(|(addr, quote, _)| (addr, quote)))
    }
}

async fn fetch_initialized_pools(
    rpc: &ArchRpcClient,
    token_a: Pubkey,
    token_b: Pubkey,
) -> Result<Vec<InitializedPool>> {
    let pools = fetch_whirlpools_by_token_pair(rpc, token_a, token_b)
        .await
        .map_err(|e| anyhow::anyhow!("failed to fetch pools: {}", e))?;

    let initialized: Vec<InitializedPool> = pools
        .into_iter()
        .filter_map(|p| match p {
            PoolInfo::Initialized(pool) => Some(pool),
            PoolInfo::Uninitialized(_) => None,
        })
        .collect();

    let key = sort_pair(token_a, token_b);
    tracing::info!(
        "Discovered {} initialized pool(s) for {:?} <-> {:?}",
        initialized.len(),
        key.0,
        key.1,
    );

    Ok(initialized)
}
