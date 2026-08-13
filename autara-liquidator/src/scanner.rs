use arch_sdk::{
    AsyncArchRpcClient, Status,
    arch_program::{
        bitcoin::{Network, key::Keypair},
        pubkey::Pubkey,
    },
};
use autara_client::client::{
    blockhash_cache::BlockhashCache, read::AutaraReadClient,
    single_thread_client::AutaraReadClientImpl, tx_broadcast::AutaraTxBroadcast,
    tx_builder::AutaraTransactionBuilder,
};

use crate::{
    balances::{positive_balance_delta, rate_within_slippage, read_token_balance},
    config::TokenFilter,
    propamm::{PropAmmClient, RfqQuote},
    router::{ClammExecution, SwapRouter},
    venue::{Venue, VenueQuote},
};

const VENUE_QUOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn choose_venue(clamm_out: Option<u64>, propamm_out: Option<u64>) -> Option<Venue> {
    let clamm_out = clamm_out.filter(|amount| *amount > 0);
    let propamm_out = propamm_out.filter(|amount| *amount > 0);
    match (clamm_out, propamm_out) {
        (Some(clamm), Some(propamm)) if propamm > clamm => Some(Venue::PropAmm),
        (Some(_), Some(_)) | (Some(_), None) => Some(Venue::Clamm),
        (None, Some(_)) => Some(Venue::PropAmm),
        (None, None) => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn scan_liquidatable_positions(
    client: &AutaraReadClientImpl,
    router: Option<&SwapRouter>,
    propamm: Option<&PropAmmClient>,
    token_filter: &TokenFilter,
    arch_client: &AsyncArchRpcClient,
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
        let Some(market_wrapper) = client.get_market(market_key) else {
            continue;
        };
        let supply_mint = market_wrapper.market().supply_token_info().mint;
        let collateral_mint = market_wrapper.market().collateral_token_info().mint;
        if !token_filter.allows_market(&supply_mint, &collateral_mint) {
            continue;
        }
        let Ok(health) = market_wrapper.borrow_position_health(&borrow_position) else {
            continue;
        };
        if biggest_borrow
            .as_ref()
            .is_none_or(|(_, _, atoms)| health.borrowed_atoms > *atoms)
        {
            biggest_borrow = Some((position_key, *market_key, health.borrowed_atoms));
        }
        if highest_ltv
            .as_ref()
            .is_none_or(|(_, _, ltv)| health.ltv > *ltv)
        {
            highest_ltv = Some((position_key, *market_key, health.ltv));
        }

        let unhealthy_ltv = market_wrapper.market().config().ltv_config().unhealthy_ltv;
        if health.ltv < unhealthy_ltv {
            continue;
        }
        liquidatable_count += 1;
        tracing::info!(
            ?position_key,
            authority = ?borrow_position.authority(),
            ?market_key,
            ltv = %health.ltv,
            unhealthy_ltv = %unhealthy_ltv,
            borrowed_atoms = health.borrowed_atoms,
            collateral_atoms = health.collateral_atoms,
            "LIQUIDATABLE"
        );

        let collateral_atoms = match market_wrapper.market().compute_liquidation_result_with_fee(
            &borrow_position,
            market_wrapper.collateral_oracle(),
            market_wrapper.supply_oracle(),
            u64::MAX,
        ) {
            Ok((_health_before, liquidation)) => {
                match liquidation.total_collateral_atoms_to_liquidate() {
                    Ok(amount) if amount > 0 => amount,
                    Ok(_) => {
                        tracing::warn!(?position_key, "Liquidation would seize zero collateral");
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(?position_key, %error, "Failed to compute seized collateral");
                        continue;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(?position_key, %error, "Failed to preview liquidation");
                continue;
            }
        };

        let (clamm_quote, propamm_quote) = quote_venues(
            router,
            propamm,
            collateral_mint,
            supply_mint,
            collateral_atoms,
            signer,
        )
        .await;
        let selected = choose_venue(
            clamm_quote.as_ref().map(|quote| quote.estimated_out),
            propamm_quote.as_ref().map(|quote| quote.estimated_out),
        );
        tracing::info!(
            ?position_key,
            collateral_in = collateral_atoms,
            clamm_out = clamm_quote.as_ref().map(|quote| quote.estimated_out),
            propamm_out = propamm_quote.as_ref().map(|quote| quote.estimated_out),
            selected = ?selected,
            "ROUTE"
        );
        let Some(selected) = selected else {
            tracing::warn!(?collateral_mint, ?supply_mint, "No valid liquidation route");
            continue;
        };
        if dry_run {
            tracing::info!(
                ?position_key,
                ?market_key,
                ?selected,
                "DRY-RUN: not broadcasting"
            );
            continue;
        }

        match selected {
            Venue::Clamm => {
                let Some(quote) = clamm_quote else {
                    continue;
                };
                execute_clamm_liquidation(
                    &tx_builder,
                    &tx_broadcast,
                    market_key,
                    position_key,
                    quote,
                    keypair,
                    network,
                )
                .await;
            }
            Venue::PropAmm => {
                let Some(initial_quote) = propamm_quote else {
                    continue;
                };
                let Some(propamm) = propamm else {
                    continue;
                };
                let before = match read_token_balance(arch_client, signer, collateral_mint).await {
                    Ok(balance) => balance,
                    Err(error) => {
                        tracing::error!(?position_key, %error, "PropAMM pre-liquidation balance read failed");
                        if let Some(clamm) = clamm_quote {
                            tracing::warn!(
                                ?position_key,
                                "Falling back to CLAMM before liquidation"
                            );
                            execute_clamm_liquidation(
                                &tx_builder,
                                &tx_broadcast,
                                market_key,
                                position_key,
                                clamm,
                                keypair,
                                network,
                            )
                            .await;
                        }
                        continue;
                    }
                };
                let transaction = match tx_builder
                    .liquidate(market_key, &position_key, None, None, None)
                    .await
                {
                    Ok(transaction) => transaction,
                    Err(error) => {
                        tracing::error!(?position_key, %error, "Failed to build PropAMM-route liquidation");
                        if let Some(clamm) = clamm_quote {
                            tracing::warn!(
                                ?position_key,
                                "Falling back to CLAMM before liquidation"
                            );
                            execute_clamm_liquidation(
                                &tx_builder,
                                &tx_broadcast,
                                market_key,
                                position_key,
                                clamm,
                                keypair,
                                network,
                            )
                            .await;
                        }
                        continue;
                    }
                };
                let signed = transaction.sign(&[*keypair], network);
                if let Err(error) = tx_broadcast.broadcast_transaction(signed).await {
                    tracing::error!(
                        ?position_key,
                        %error,
                        "PropAMM-route liquidation failed or has ambiguous status; refusing fallback"
                    );
                    continue;
                }
                tracing::info!(
                    ?position_key,
                    ?market_key,
                    "Liquidation SUCCESS (PropAMM route)"
                );

                let after = match read_token_balance(arch_client, signer, collateral_mint).await {
                    Ok(balance) => balance,
                    Err(error) => {
                        inventory_alert(position_key, collateral_mint, 0, &error.to_string());
                        continue;
                    }
                };
                let Some(seized_delta) = positive_balance_delta(before, after) else {
                    inventory_alert(
                        position_key,
                        collateral_mint,
                        0,
                        "collateral balance did not increase after confirmed liquidation",
                    );
                    continue;
                };
                let fresh_quote = match propamm
                    .quote_exact_in(collateral_mint, supply_mint, seized_delta, signer)
                    .await
                {
                    Ok(Some(quote)) => quote,
                    Ok(None) => {
                        inventory_alert(
                            position_key,
                            collateral_mint,
                            seized_delta,
                            "PropAMM returned no fresh exact-delta quote",
                        );
                        continue;
                    }
                    Err(error) => {
                        inventory_alert(
                            position_key,
                            collateral_mint,
                            seized_delta,
                            &format!("fresh PropAMM quote failed: {error:#}"),
                        );
                        continue;
                    }
                };
                if !rate_within_slippage(
                    initial_quote.amount_in,
                    initial_quote.estimated_out,
                    fresh_quote.amount_in,
                    fresh_quote.estimated_out,
                    propamm.slippage_bps(),
                ) {
                    inventory_alert(
                        position_key,
                        collateral_mint,
                        seized_delta,
                        "fresh PropAMM rate exceeded the configured degradation bound",
                    );
                    continue;
                }
                let hash = match propamm
                    .execute_quote(fresh_quote.execution, keypair, network)
                    .await
                {
                    Ok(hash) => hash,
                    Err(error) => {
                        inventory_alert(
                            position_key,
                            collateral_mint,
                            seized_delta,
                            &format!("PropAMM RFQ submission failed: {error:#}"),
                        );
                        continue;
                    }
                };
                match arch_client.wait_for_processed_transaction(&hash).await {
                    Ok(processed) if processed.status == Status::Processed => tracing::info!(
                        ?position_key,
                        collateral_in = seized_delta,
                        estimated_supply_out = fresh_quote.estimated_out,
                        tx_hash = %hash,
                        "PropAMM swap SUCCESS"
                    ),
                    Ok(processed) => inventory_alert(
                        position_key,
                        collateral_mint,
                        seized_delta,
                        &format!("PropAMM swap status was {:?}", processed.status),
                    ),
                    Err(error) => inventory_alert(
                        position_key,
                        collateral_mint,
                        seized_delta,
                        &format!("PropAMM swap confirmation failed: {error}"),
                    ),
                }
            }
        }
    }

    if liquidatable_count > 0 {
        tracing::info!(liquidatable_count, "Found liquidatable positions");
    } else {
        tracing::info!("No liquidatable positions found");
    }
    if let Some((position, market, borrowed_atoms)) = biggest_borrow {
        tracing::info!(?position, ?market, borrowed_atoms, "STATS biggest_borrow");
    }
    if let Some((position, market, ltv)) = highest_ltv {
        tracing::info!(?position, ?market, %ltv, "STATS highest_ltv");
    }
}

async fn quote_venues(
    router: Option<&SwapRouter>,
    propamm: Option<&PropAmmClient>,
    input_mint: Pubkey,
    output_mint: Pubkey,
    amount_in: u64,
    signer: Pubkey,
) -> (
    Option<VenueQuote<ClammExecution>>,
    Option<VenueQuote<RfqQuote>>,
) {
    let clamm = async {
        let router = router?;
        match tokio::time::timeout(
            VENUE_QUOTE_TIMEOUT,
            router.best_quote_exact_in(input_mint, output_mint, amount_in, signer),
        )
        .await
        {
            Ok(Ok(quote)) => quote,
            Ok(Err(error)) => {
                tracing::warn!(%error, "CLAMM quote failed");
                None
            }
            Err(_) => {
                tracing::warn!("CLAMM quote timed out");
                None
            }
        }
    };
    let propamm = async {
        let propamm = propamm?;
        match tokio::time::timeout(
            VENUE_QUOTE_TIMEOUT,
            propamm.quote_exact_in(input_mint, output_mint, amount_in, signer),
        )
        .await
        {
            Ok(Ok(quote)) => quote,
            Ok(Err(error)) => {
                tracing::warn!(%error, "PropAMM RFQ quote failed");
                None
            }
            Err(_) => {
                tracing::warn!("PropAMM RFQ quote timed out");
                None
            }
        }
    };
    tokio::join!(clamm, propamm)
}

#[allow(clippy::too_many_arguments)]
async fn execute_clamm_liquidation(
    tx_builder: &AutaraTransactionBuilder<'_, AutaraReadClientImpl>,
    tx_broadcast: &AutaraTxBroadcast<'_>,
    market_key: &Pubkey,
    position_key: Pubkey,
    quote: VenueQuote<ClammExecution>,
    keypair: &Keypair,
    network: Network,
) {
    let pool = quote.execution.pool;
    let transaction = match tx_builder
        .liquidate(
            market_key,
            &position_key,
            None,
            None,
            Some(quote.execution.callback),
        )
        .await
    {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::error!(?position_key, ?pool, %error, "Failed to build CLAMM liquidation");
            return;
        }
    };
    let signed = transaction.sign(&[*keypair], network);
    match tx_broadcast.broadcast_transaction(signed).await {
        Ok(events) => tracing::info!(
            ?position_key,
            ?market_key,
            ?pool,
            ?events,
            "Liquidation SUCCESS (CLAMM callback)"
        ),
        Err(error) => tracing::error!(
            ?position_key,
            ?market_key,
            ?pool,
            %error,
            "CLAMM liquidation FAILED"
        ),
    }
}

fn inventory_alert(position: Pubkey, mint: Pubkey, amount: u64, reason: &str) {
    tracing::error!(
        ?position,
        ?mint,
        amount,
        reason,
        "INVENTORY ALERT: liquidation landed but PropAMM settlement did not complete; refusing CLAMM fallback"
    );
}

#[cfg(test)]
mod tests {
    use crate::venue::Venue;

    use super::choose_venue;

    #[test]
    fn chooses_available_or_best_venue_and_prefers_atomic_ties() {
        assert_eq!(choose_venue(Some(10), None), Some(Venue::Clamm));
        assert_eq!(choose_venue(None, Some(10)), Some(Venue::PropAmm));
        assert_eq!(choose_venue(Some(11), Some(10)), Some(Venue::Clamm));
        assert_eq!(choose_venue(Some(10), Some(11)), Some(Venue::PropAmm));
        assert_eq!(choose_venue(Some(10), Some(10)), Some(Venue::Clamm));
        assert_eq!(choose_venue(Some(0), Some(0)), None);
        assert_eq!(choose_venue(None, None), None);
    }
}
