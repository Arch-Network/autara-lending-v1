use std::sync::Arc;

use arch_sdk::ArchRpcClient;
use arch_sdk::arch_program::bitcoin::Network;
use arch_sdk::arch_program::bitcoin::key::Keypair;
use arch_sdk::arch_program::pubkey::Pubkey;
use autara_client::client::blockhash_cache::BlockhashCache;
use autara_client::client::read::AutaraReadClient;
use autara_client::client::single_thread_client::AutaraReadClientImpl;
use autara_client::client::tx_broadcast::AutaraTxBroadcast;
use autara_client::client::tx_builder::AutaraTransactionBuilder;
use orca_whirlpools::SwapQuote;

use crate::config::TokenFilter;
use crate::router::SwapRouter;

pub async fn scan_liquidatable_positions(
    client: &AutaraReadClientImpl,
    router: &Arc<SwapRouter>,
    propamm: Option<&crate::propamm::PropAmm>,
    token_filter: &TokenFilter,
    arch_client: &ArchRpcClient,
    autara_program_id: Pubkey,
    keypair: &Keypair,
    signer: Pubkey,
    blockhash_cache: &BlockhashCache,
    network: Network,
    dry_run: bool,
) {
    let mut liquidatable_count = 0u64;

    let mut biggest_borrow: Option<(Pubkey, Pubkey, u64)> = None;
    let mut highest_ltv: Option<(Pubkey, Pubkey, autara_lib::math::ifixed_point::IFixedPoint)> =
        None;

    let tx_builder = AutaraTransactionBuilder {
        arch_client,
        autara_read_client: client,
        autara_program_id,
        authority_key: signer,
        blockhash_cache: Some(blockhash_cache),
    };

    let tx_broadcast = AutaraTxBroadcast {
        program_id: &autara_program_id,
        arch_client,
    };

    for (position_key, borrow_position) in client.all_borrow_position() {
        let market_key = borrow_position.market();
        let market_wrapper = match client.get_market(market_key) {
            Some(mw) => mw,
            None => continue,
        };

        let supply_mint = market_wrapper.market().supply_token_info().mint;
        let collateral_mint = market_wrapper.market().collateral_token_info().mint;

        if !token_filter.allows_market(&supply_mint, &collateral_mint) {
            continue;
        }

        let health = match market_wrapper.borrow_position_health(&borrow_position) {
            Ok(h) => h,
            Err(_) => continue,
        };

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

        if health.ltv >= unhealthy_ltv {
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

            // Find a swap route: collateral -> supply (to repay debt).
            //
            // Size the swap to the collateral the liquidation will ACTUALLY seize
            // (collateral_atoms_to_liquidate + liquidation bonus), NOT the full position
            // collateral. We call liquidate() below with max_repay = u64::MAX, so we preview
            // the same computation here:
            //   * ltv >= 1 (bad debt)  -> seizes the full collateral (full liquidation),
            //   * unhealthy <= ltv < 1 -> seizes only a PARTIAL amount (down to target LTV).
            // Selling the full collateral on a partial liquidation would oversell tokens the
            // liquidator never receives, making the liquidate tx revert (or bleed inventory).
            let collateral_atoms = match market_wrapper.market().compute_liquidation_result_with_fee(
                &borrow_position,
                market_wrapper.collateral_oracle(),
                market_wrapper.supply_oracle(),
                u64::MAX,
            ) {
                Ok((_health_before, liquidation)) => {
                    match liquidation.total_collateral_atoms_to_liquidate() {
                        Ok(seized) if seized > 0 => seized,
                        Ok(_) => {
                            tracing::warn!(
                                "Liquidation would seize 0 collateral for position={:?}; skipping",
                                position_key,
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to compute seized collateral: {:#}", e);
                            continue;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to preview liquidation result: {:#}", e);
                    continue;
                }
            };

            // ---- Quote BOTH venues for collateral -> supply of the seized amount; route to best ----
            // CLAMM (whirlpool) on-chain quote.
            let clamm: Option<(Pubkey, orca_whirlpools::SwapInstructions, u64)> =
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    router.best_quote_exact_in(collateral_mint, supply_mint, collateral_atoms, Some(signer)),
                )
                .await
                {
                    Ok(Ok(Some((pool, swap_ix)))) => {
                        let est_out = match &swap_ix.quote {
                            SwapQuote::ExactIn(q) => q.token_est_out,
                            SwapQuote::ExactOut(_) => 0,
                        };
                        Some((pool, swap_ix, est_out))
                    }
                    Ok(Ok(None)) => None,
                    Ok(Err(e)) => {
                        tracing::warn!("CLAMM quote failed: {:#}", e);
                        None
                    }
                    Err(_) => {
                        tracing::warn!("CLAMM quote timed out");
                        None
                    }
                };
            let clamm_out = clamm.as_ref().map(|(_, _, o)| *o).unwrap_or(0);

            // PropAMM (RFQ vault) quote — only if configured and it supports this pair.
            let propamm_quote: Option<(f64, u64)> = match propamm {
                Some(p) if p.supports(&collateral_mint, &supply_mint) => match p.fetch_price().await {
                    Ok(price) => p
                        .quote(&collateral_mint, &supply_mint, collateral_atoms, price)
                        .map(|(_, _, _, out)| (price, out)),
                    Err(e) => {
                        tracing::warn!("PropAMM price fetch failed: {:#}", e);
                        None
                    }
                },
                _ => None,
            };
            let propamm_out = propamm_quote.map(|(_, o)| o).unwrap_or(0);

            tracing::info!(
                "ROUTE position={:?} collateral_in={} clamm_out={} propamm_out={} -> {}",
                position_key,
                collateral_atoms,
                clamm_out,
                propamm_out,
                if propamm_out > clamm_out { "PropAMM" } else { "CLAMM" },
            );

            if clamm_out == 0 && propamm_out == 0 {
                tracing::warn!(
                    "No route on any venue for {:?} -> {:?}; skipping",
                    collateral_mint,
                    supply_mint,
                );
                continue;
            }

            let use_propamm = propamm_out > clamm_out;

            if dry_run {
                tracing::info!(
                    "DRY-RUN: would liquidate position={:?} market={:?} via {}; not broadcasting",
                    position_key,
                    market_key,
                    if use_propamm { "PropAMM" } else { "CLAMM" },
                );
                continue;
            }

            if use_propamm {
                // Decoupled path: PropAMM cannot be an atomic liquidate callback (its quote_signer
                // must co-sign), so liquidate WITHOUT a callback (repay from the bot's float) and
                // then swap the seized collateral on PropAMM in a separate tx.
                let tx_to_sign = match tx_builder
                    .liquidate(market_key, &position_key, None, None, None)
                    .await
                {
                    Ok(tx) => tx,
                    Err(e) => {
                        tracing::error!("Failed to build liquidate tx: {:#}", e);
                        continue;
                    }
                };
                let signed_tx = tx_to_sign.sign(&[keypair.clone()], network);
                match tx_broadcast.broadcast_transaction(signed_tx).await {
                    Ok(events) => tracing::info!(
                        "Liquidation SUCCESS (no-callback, PropAMM route) position={:?} market={:?} events={:?}",
                        position_key,
                        market_key,
                        events,
                    ),
                    Err(e) => {
                        tracing::error!(
                            "Liquidation FAILED for position={:?} market={:?}: {:#}",
                            position_key,
                            market_key,
                            e,
                        );
                        continue;
                    }
                }
                // Swap the just-seized collateral -> supply on PropAMM.
                let p = propamm.expect("propamm route implies propamm configured");
                let price = propamm_quote.expect("propamm route implies a quote").0;
                match p
                    .execute_swap(arch_client, keypair, signer, &collateral_mint, &supply_mint, collateral_atoms, price, network)
                    .await
                {
                    Ok(out) => tracing::info!(
                        "PropAMM swap SUCCESS position={:?} collateral_in={} supply_out~{}",
                        position_key,
                        collateral_atoms,
                        out,
                    ),
                    Err(e) => tracing::error!(
                        "PropAMM swap FAILED after liquidation (seized collateral held by liquidator) position={:?}: {:#}",
                        position_key,
                        e,
                    ),
                }
            } else {
                // Atomic path: CLAMM swap as a CPI callback inside the liquidate instruction.
                let (_pool, swap_ix, _) = clamm.expect("clamm route implies a clamm quote");
                let ix_callback = swap_ix.instructions.into_iter().next();
                let tx_to_sign = match tx_builder
                    .liquidate(market_key, &position_key, None, None, ix_callback)
                    .await
                {
                    Ok(tx) => tx,
                    Err(e) => {
                        tracing::error!("Failed to build liquidate tx: {:#}", e);
                        continue;
                    }
                };
                let signed_tx = tx_to_sign.sign(&[keypair.clone()], network);
                match tx_broadcast.broadcast_transaction(signed_tx).await {
                    Ok(events) => tracing::info!(
                        "Liquidation SUCCESS (CLAMM callback) position={:?} market={:?} events={:?}",
                        position_key,
                        market_key,
                        events,
                    ),
                    Err(e) => tracing::error!(
                        "Liquidation FAILED for position={:?} market={:?}: {:#}",
                        position_key,
                        market_key,
                        e,
                    ),
                }
            }
        }
    }

    if liquidatable_count > 0 {
        tracing::info!("Found {} liquidatable position(s)", liquidatable_count);
    } else {
        tracing::info!("No liquidatable positions found");
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
