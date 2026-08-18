//! Individual, flag-gated deploy steps. Each step is a thin wrapper that builds
//! the relevant instruction(s) via `autara-lib` and sends them through the
//! shared [`RpcContext`], recording tx ids into the [`DeploymentArtifact`].

use anyhow::{anyhow, bail, Context, Result};
use apl_token::state::{Account as TokenAccount, Mint};
use arch_program::program_option::COption;
use arch_program::program_pack::Pack;
use arch_program::pubkey::Pubkey;

use autara_lib::interest_rate::interest_rate_kind::InterestRateCurveKind;
use autara_lib::ixs::{create_market_ix, CreateMarketInstruction};
use autara_lib::math::ifixed_point::IFixedPoint;
use autara_lib::oracle::oracle_config::OracleConfig;
use autara_lib::pda::find_market_pda;
use autara_lib::state::market_config::LtvConfig;
use autara_lib::token::{create_ata_ix, get_associated_token_address};

use crate::artifact::{DeploymentArtifact, MarketRecord};
use crate::config::{pyth_feed_for_label, MarketPair, MarketParams, TokenConfig};
use crate::rpc::RpcContext;

/// Create the protocol's global config PDA (admin + fee receiver + fee share).
///
/// Idempotent: if the global config already exists on-chain, the existing PDA
/// is returned and no new transaction is recorded.
pub async fn create_global_config(
    ctx: &RpcContext,
    autara_program_id: Pubkey,
    admin: Pubkey,
    fee_receiver: Pubkey,
    protocol_fee_share_bps: u16,
    artifact: &mut DeploymentArtifact,
) -> Result<Pubkey> {
    let payer = ctx.payer_pubkey();
    let (global_config_pda, ix) = autara_lib::ixs::create_global_config_ix(
        autara_program_id,
        payer,
        admin,
        fee_receiver,
        protocol_fee_share_bps,
    );

    match ctx.send(vec![ix], vec![]).await {
        Ok(txid) => {
            artifact.record_tx("create_global_config", txid);
        }
        Err(e) if e.to_string().contains("already exists") => {
            println!("global_config:     already exists ({global_config_pda}) — skipping");
        }
        Err(e) => return Err(e),
    }

    artifact.global_config = Some(global_config_pda.to_string());
    Ok(global_config_pda)
}

/// Token-setup step: ENSURE every configured token mint is present on-chain.
///
/// The deploy tool intentionally carries token *pubkeys* only (never the mint
/// authority secret), so it does not create mints — those are produced once by
/// `autara-cli token setup`. This step is the idempotent guard that the mints
/// exist before any market is created; it fails loudly (with a pointer to the
/// CLI) rather than letting `create_market` fail with a confusing on-chain
/// error. Mints are already recorded in the artifact's `tokens` list.
pub async fn ensure_token_mints(ctx: &RpcContext, tokens: &[TokenConfig]) -> Result<()> {
    for token in tokens {
        if ctx.account_exists(token.mint).await {
            println!("token {:<6}     mint {} present", token.label, token.mint);
        } else {
            bail!(
                "token '{}' mint {} is not on-chain — create the APL mints first \
                 (e.g. `autara-cli token setup`); the deploy tool holds no mint authority",
                token.label,
                token.mint
            );
        }
    }
    Ok(())
}

/// Mint a token's configured initial supply to `recipient`'s ATA, signed/paid by
/// the mint authority (`authority_ctx`'s payer).
///
/// SAFETY GATE (mirrors CLAMM's `clamm-deploy mint-to`): before sending anything
/// it reads the mint on-chain and REFUSES if the on-chain `mint_authority` does
/// not match the supplied authority keypair, or if the mint's `decimals` differ
/// from the configured value. This makes a wrong-key run a no-op error rather
/// than a silent failure, so the candidate authority keys can be supplied at the
/// gated live run without risk.
///
/// Idempotent-ish: if the recipient ATA already holds `>= mint_amount`, the mint
/// is skipped (so a re-run does not keep inflating supply).
pub async fn mint_initial_supply(
    authority_ctx: &RpcContext,
    token: &TokenConfig,
    recipient: Pubkey,
    artifact: &mut DeploymentArtifact,
) -> Result<()> {
    let authority = authority_ctx.payer_pubkey();

    // ----- 1. read-only safety gate: decimals + on-chain mint_authority -----
    let mint_info = authority_ctx
        .rpc
        .read_account_info(token.mint)
        .await
        .map_err(|e| anyhow!("reading mint {} ({}) failed: {e}", token.label, token.mint))?;
    let mint_state = Mint::unpack(&mint_info.data)
        .map_err(|e| anyhow!("decoding mint {} ({}): {e}", token.label, token.mint))?;

    if mint_state.decimals != token.decimals {
        bail!(
            "decimals mismatch for {} ({}): on-chain {}, configured {}",
            token.label,
            token.mint,
            mint_state.decimals,
            token.decimals
        );
    }
    match mint_state.mint_authority {
        COption::Some(on_chain) if on_chain == authority => {}
        COption::Some(on_chain) => bail!(
            "mint_authority MISMATCH for {} ({}): on-chain {on_chain}, supplied {authority}. \
             Refusing to mint — supply the correct MINT_AUTHORITY_KEY_PATH[_{}].",
            token.label,
            token.mint,
            token.label.to_uppercase()
        ),
        COption::None => bail!(
            "mint {} ({}) has no mint_authority (fixed supply); cannot mint",
            token.label,
            token.mint
        ),
    }

    // ----- 2. idempotency: skip if the recipient ATA already holds enough -----
    let ata = get_associated_token_address(&recipient, &token.mint);
    let existing_balance = match authority_ctx.rpc.read_account_info(ata).await {
        Ok(info) => TokenAccount::unpack(&info.data).map(|a| a.amount).ok(),
        Err(_) => None,
    };
    if matches!(existing_balance, Some(bal) if bal >= token.mint_amount) {
        println!(
            "mint {:<6}     {} already holds {} (>= {}) — skipping",
            token.label,
            ata,
            existing_balance.unwrap(),
            token.mint_amount
        );
        return Ok(());
    }

    // ----- 3. ensure the recipient ATA exists (funded by the authority) -----
    if existing_balance.is_none() {
        let create_ix = create_ata_ix(&authority, Some(&ata), &recipient, &token.mint);
        let txid = authority_ctx
            .send(vec![create_ix], vec![])
            .await
            .with_context(|| format!("creating ATA {ata} for {}", token.label))?;
        artifact.record_tx(&format!("create_ata:{}", token.label), txid);
    }

    // ----- 4. mint the configured initial supply -----
    let mint_ix = apl_token::instruction::mint_to(
        &apl_token::id(),
        &token.mint,
        &ata,
        &authority,
        &[],
        token.mint_amount,
    )
    .map_err(|e| anyhow!("building mint_to for {} failed: {e}", token.label))?;
    let txid = authority_ctx
        .send(vec![mint_ix], vec![])
        .await
        .with_context(|| format!("mint_to {} ({})", token.label, token.mint))?;
    artifact.record_tx(&format!("mint_initial_supply:{}", token.label), txid);
    println!(
        "mint {:<6}     {} += {} -> {}",
        token.label, ata, token.mint_amount, recipient
    );
    Ok(())
}

/// Build a `CreateMarketInstruction` from the env-configured market parameters.
///
/// `MarketParams::default()` reproduces `autara-server`'s `default_market_config`
/// (max_ltv 0.8 / unhealthy 0.9 / liquidation bonus 0.05, 90% max utilisation),
/// so an unset env yields byte-for-byte identical instructions (the `f64` values
/// flow through `IFixedPoint::from_num` exactly as the old literals did — see
/// `tests::default_params_match_legacy_hardcoded_values`). The interest-rate
/// curve is always adaptive (not parameterized).
fn build_create_market_instruction(
    index: u8,
    lending_market_fee_bps: u16,
    params: MarketParams,
    supply_oracle: OracleConfig,
    collateral_oracle: OracleConfig,
) -> CreateMarketInstruction {
    CreateMarketInstruction {
        // `market_bump` is overwritten by `create_market_ix` from the derived PDA.
        market_bump: 0,
        index,
        ltv_config: LtvConfig {
            max_ltv: IFixedPoint::from_num(params.max_ltv),
            unhealthy_ltv: IFixedPoint::from_num(params.unhealthy_ltv),
            liquidation_bonus: IFixedPoint::from_num(params.liquidation_bonus),
        },
        max_utilisation_rate: IFixedPoint::from_num(params.max_utilisation_rate),
        supply_oracle_config: supply_oracle,
        collateral_oracle_config: collateral_oracle,
        interest_rate: InterestRateCurveKind::new_adaptive(),
        lending_market_fee_in_bps: lending_market_fee_bps,
    }
}

/// Derive the market PDA for a pair without sending anything (used by the
/// dry-run preview and by [`create_market`]).
pub fn derive_market_pda(
    autara_program_id: Pubkey,
    curator: Pubkey,
    supply: &TokenConfig,
    collateral: &TokenConfig,
    index: u8,
) -> Pubkey {
    find_market_pda(
        &autara_program_id,
        &curator,
        &supply.mint,
        &collateral.mint,
        index,
    )
    .0
}

/// Create a lending market for one supply/collateral pair (curator = admin).
///
/// Idempotent: if the market PDA already exists it is recorded with
/// `created=false` and no transaction is sent. Reuses
/// `autara_lib::ixs::create_market_ix` so the on-chain layout cannot drift.
#[allow(clippy::too_many_arguments)]
pub async fn create_market(
    ctx: &RpcContext,
    autara_program_id: Pubkey,
    oracle_program_id: Pubkey,
    curator: Pubkey,
    pair: &MarketPair,
    supply: &TokenConfig,
    collateral: &TokenConfig,
    lending_market_fee_bps: u16,
    market_params: MarketParams,
    index: u8,
    artifact: &mut DeploymentArtifact,
) -> Result<Pubkey> {
    let supply_feed = pyth_feed_for_label(&supply.label)
        .ok_or_else(|| anyhow::anyhow!("no Pyth feed for supply token '{}'", supply.label))?;
    let collateral_feed = pyth_feed_for_label(&collateral.label).ok_or_else(|| {
        anyhow::anyhow!("no Pyth feed for collateral token '{}'", collateral.label)
    })?;

    let market_pda = derive_market_pda(autara_program_id, curator, supply, collateral, index);
    let label = format!(
        "create_market:{}/{}",
        pair.supply_label, pair.collateral_label
    );

    if ctx.account_exists(market_pda).await {
        println!(
            "market {}/{} already exists ({market_pda}) — skipping",
            pair.supply_label, pair.collateral_label
        );
        artifact.record_market(MarketRecord {
            supply_label: pair.supply_label.clone(),
            collateral_label: pair.collateral_label.clone(),
            supply_mint: supply.mint.to_string(),
            collateral_mint: collateral.mint.to_string(),
            index,
            market: market_pda.to_string(),
            created: false,
        });
        return Ok(market_pda);
    }

    let create_market = build_create_market_instruction(
        index,
        lending_market_fee_bps,
        market_params,
        OracleConfig::new_pyth(supply_feed, oracle_program_id),
        OracleConfig::new_pyth(collateral_feed, oracle_program_id),
    );
    let (derived, ix) = create_market_ix(
        create_market,
        supply.mint,
        collateral.mint,
        autara_program_id,
        curator,
        ctx.payer_pubkey(),
    );
    debug_assert_eq!(derived, market_pda);

    let txid = ctx.send(vec![ix], vec![]).await?;
    artifact.record_tx(&label, txid);
    artifact.record_market(MarketRecord {
        supply_label: pair.supply_label.clone(),
        collateral_label: pair.collateral_label.clone(),
        supply_mint: supply.mint.to_string(),
        collateral_mint: collateral.mint.to_string(),
        index,
        market: market_pda.to_string(),
        created: true,
    });
    Ok(market_pda)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_program::pubkey::Pubkey;

    /// Proof that making the market params env-configurable did NOT change
    /// testnet behavior: with `MarketParams::default()` the built instruction is
    /// byte-for-byte identical to the previously hardcoded values
    /// (max_ltv 0.8 / unhealthy 0.9 / liquidation_bonus 0.05 / max_util 0.9).
    #[test]
    fn default_params_match_legacy_hardcoded_values() {
        let oracle = OracleConfig::new_pyth([0u8; 32], Pubkey::new_from_array([0u8; 32]));
        let ix = build_create_market_instruction(0, 100, MarketParams::default(), oracle, oracle);
        assert_eq!(ix.ltv_config.max_ltv, IFixedPoint::from_num(0.8));
        assert_eq!(ix.ltv_config.unhealthy_ltv, IFixedPoint::from_num(0.9));
        assert_eq!(ix.ltv_config.liquidation_bonus, IFixedPoint::from_num(0.05));
        assert_eq!(ix.max_utilisation_rate, IFixedPoint::from_num(0.9));
    }
}
