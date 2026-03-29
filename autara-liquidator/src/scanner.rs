use std::sync::Arc;

use arch_sdk::ArchRpcClient;
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
    token_filter: &TokenFilter,
    arch_client: &ArchRpcClient,
    autara_program_id: Pubkey,
    keypair: &Keypair,
    signer: Pubkey,
    blockhash_cache: &BlockhashCache,
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

            // Find a swap route: collateral -> supply (to repay debt)
            let collateral_atoms = health.collateral_atoms;

            let quote_result = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                router.best_quote_exact_in(
                    collateral_mint,
                    supply_mint,
                    collateral_atoms,
                    Some(signer),
                ),
            )
            .await
            {
                Ok(result) => Some(result),
                Err(_) => {
                    tracing::warn!(
                        "Swap quote timed out for {:?} -> {:?}",
                        collateral_mint,
                        supply_mint,
                    );
                    None
                }
            };

            let swap_ix = match quote_result {
                Some(Ok(Some((_pool, swap_ix)))) => {
                    let (est_out, min_out) = match &swap_ix.quote {
                        SwapQuote::ExactIn(q) => (q.token_est_out, q.token_min_out),
                        SwapQuote::ExactOut(_) => unreachable!(),
                    };
                    tracing::info!(
                        "ROUTE found: pool={:?} collateral_in={} supply_est_out={} supply_min_out={}",
                        _pool,
                        collateral_atoms,
                        est_out,
                        min_out,
                    );
                    Some(swap_ix)
                }
                Some(Ok(None)) => {
                    tracing::warn!(
                        "No swap route found for {:?} -> {:?}",
                        collateral_mint,
                        supply_mint,
                    );
                    continue;
                }
                Some(Err(e)) => {
                    tracing::warn!("Failed to get swap quote: {:#}", e);
                    continue;
                }
                None => continue,
            };

            // Build and send the liquidate transaction
            // The swap callback instruction is passed to liquidate so the program
            // can execute the swap as part of the liquidation.
            let ix_callback = swap_ix.and_then(|si| si.instructions.into_iter().next());

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

            // let network = arch_sdk::arch_program::bitcoin::Network::Regtest;
            // let signed_tx = tx_to_sign.sign(&[keypair.clone()], network);

            // match tx_broadcast.broadcast_transaction(signed_tx).await {
            //     Ok(events) => {
            //         tracing::info!(
            //             "Liquidation SUCCESS for position={:?} market={:?} events={:?}",
            //             position_key,
            //             market_key,
            //             events,
            //         );
            //     }
            //     Err(e) => {
            //         tracing::error!(
            //             "Liquidation FAILED for position={:?} market={:?}: {:#}",
            //             position_key,
            //             market_key,
            //             e,
            //         );
            //     }
            // }
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
