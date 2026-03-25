use std::collections::HashMap;

use arch_sdk::arch_program::pubkey::Pubkey;
use autara_client::client::{read::AutaraReadClient, single_thread_client::AutaraReadClientImpl};
use orca_whirlpools::SwapQuote;

use crate::config::TokenFilter;
use crate::router::SwapRouter;

pub async fn scan_liquidatable_positions(
    client: &AutaraReadClientImpl,
    router: &SwapRouter,
    token_filter: &TokenFilter,
    signer: Pubkey,
) {
    let mut liquidatable_count = 0u64;

    // Track biggest borrow obligation and highest LTV across all positions
    let mut biggest_borrow: Option<(Pubkey, Pubkey, u64)> = None; // (position, market, borrowed_atoms)
    let mut highest_ltv: Option<(Pubkey, Pubkey, autara_lib::math::ifixed_point::IFixedPoint)> =
        None; // (position, market, ltv)

    // Cache route availability per mint pair to avoid redundant RPC calls
    let mut route_cache: HashMap<(Pubkey, Pubkey), bool> = HashMap::new();

    for (position_key, borrow_position) in client.all_borrow_position() {
        let market_key = borrow_position.market();
        let market_wrapper = match client.get_market(market_key) {
            Some(mw) => mw,
            None => {
                continue;
            }
        };

        let supply_mint = market_wrapper.market().supply_token_info().mint;
        let collateral_mint = market_wrapper.market().collateral_token_info().mint;

        if !token_filter.allows_market(&supply_mint, &collateral_mint) {
            continue;
        }

        let health = match market_wrapper.borrow_position_health(&borrow_position) {
            Ok(h) => h,
            Err(_) => {
                continue;
            }
        };

        // Track biggest borrow and highest LTV
        match &biggest_borrow {
            None => biggest_borrow = Some((position_key, *market_key, health.borrowed_atoms)),
            Some((_, _, prev_atoms)) if health.borrowed_atoms > *prev_atoms => {
                biggest_borrow = Some((position_key, *market_key, health.borrowed_atoms));
            }
            _ => {}
        }
        match &highest_ltv {
            None => highest_ltv = Some((position_key, *market_key, health.ltv)),
            Some((_, _, prev_ltv)) if health.ltv > *prev_ltv => {
                highest_ltv = Some((position_key, *market_key, health.ltv));
            }
            _ => {}
        }

        let unhealthy_ltv = market_wrapper.market().config().ltv_config().unhealthy_ltv;

        if health.ltv >= unhealthy_ltv || true {
            liquidatable_count += 1;

            tracing::info!(
                "LIQUIDATABLE position={:?} authority={:?} market={:?} ltv={} unhealthy_ltv={} borrowed_atoms={} collateral_atoms={}",
                position_key,
                borrow_position.authority(),
                market_key,
                health.ltv,
                unhealthy_ltv,
                health.borrowed_atoms,
                health.collateral_atoms,
            );

            // Try to find a swap route: collateral -> supply (to repay debt)
            let quote_result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                router.best_quote_exact_in(
                    collateral_mint,
                    supply_mint,
                    health.collateral_atoms,
                    Some(signer),
                ),
            )
            .await;

            match quote_result {
                Err(_) => {
                    tracing::warn!(
                        "  Swap quote timed out for {:?} -> {:?}",
                        collateral_mint,
                        supply_mint
                    );
                }
                Ok(Ok(Some((pool, quote)))) => {
                    let (est_out, min_out) = match &quote {
                        SwapQuote::ExactIn(q) => (q.token_est_out, q.token_min_out),
                        SwapQuote::ExactOut(_) => unreachable!(),
                    };
                    tracing::info!(
                        "  ROUTE found: pool={:?} collateral_in={} supply_est_out={} supply_min_out={}",
                        pool,
                        health.collateral_atoms,
                        est_out,
                        min_out,
                    );
                }
                Ok(Ok(None)) => {
                    tracing::warn!(
                        "  No swap route found for {:?} -> {:?}",
                        collateral_mint,
                        supply_mint,
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!("  Failed to get swap quote: {:#}", e);
                }
            }
        }
    }

    if liquidatable_count > 0 {
        tracing::info!("Found {} liquidatable position(s)", liquidatable_count);
    } else {
        tracing::debug!("No liquidatable positions found");
    }

    if let Some((pos, market, atoms)) = biggest_borrow {
        tracing::info!(
            "STATS biggest_borrow: position={:?} market={:?} borrowed_atoms={}",
            pos,
            market,
            atoms,
        );
    }
    if let Some((pos, market, ltv)) = highest_ltv {
        tracing::info!(
            "STATS highest_ltv: position={:?} market={:?} ltv={}",
            pos,
            market,
            ltv,
        );
    }
}
