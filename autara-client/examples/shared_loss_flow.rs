// Live end-to-end proof of Autara's "shared loss" / loss-socialization feature.
//
// WHAT THIS PROVES (see docs/SHARED_LOSS.md for the full mechanism map):
//   1. A borrow position can be driven underwater (LTV >= 1) by an oracle price
//      crash, so a normal liquidation would leave bad debt.
//   2. The market curator (and ONLY the curator) can call `socialize_loss`,
//      which:
//        - writes down every supplier's redeemable balance pro-rata by the
//          full outstanding debt (the pool eats the loss),
//        - clears the bad position's debt and collateral to zero,
//        - transfers ALL of the position's collateral to the CURATOR's wallet
//          WITHOUT the curator paying anything into the pool.
//   3. The curator can later (at its sole discretion, any amount) call
//      `donate_supply` to add value back to the pool, which raises every
//      supplier's redeemable balance pro-rata. Nothing on-chain forces the
//      curator to add anything back, or to add back a "fair" amount.
//
// This is a FRESH, SELF-CONTAINED market with FRESH oracle feeds created by the
// test harness (AutaraTestEnv). It NEVER touches the shared aUSD/aBTC market.
// It runs against the Autara *stage* program on testnet by default (same target
// the integration-test suite uses).
//
// Run:
//   cargo run -p autara-client --example shared_loss_flow
//
// Env knobs (all optional; defaults give a realistic 70% LTV loan hit by a 60%
// collateral crash):
//   SL_SUPPLY_UNITS      supplier A deposit, whole supply units   (default 200000)
//   SL_COLLATERAL_UNITS  borrower B collateral, whole units       (default 1)
//   SL_BORROW_UNITS      borrower B borrow, whole supply units    (default 70000)
//   SL_COLLATERAL_PRICE  healthy collateral price in USD          (default 100000)
//   SL_CRASH_PRICE       crashed collateral price in USD          (default 40000)
//   SL_DONATE_UNITS      curator add-back after off-chain sale    (default 40000)
//
// Token mints created by the harness use 9 decimals, so 1 unit = 1e9 atoms.

use anyhow::{anyhow, Context, Result};
use arch_sdk::{
    arch_program::{bitcoin::key::Keypair, bitcoin::Network, pubkey::Pubkey},
    Status,
};
use autara_client::{
    client::{
        client_with_signer::AutaraFullClientWithSigner, read::AutaraReadClient,
        single_thread_client::AutaraReadClientImpl, tx_builder::TransactionToSign,
    },
    config::{autara_oracle_stage_program_id, autara_stage_program_id, ArchConfig},
    rpc_ext::ArchAsyncRpcExt,
    test::AutaraTestEnv,
};
use autara_lib::{
    interest_rate::interest_rate_kind::InterestRateCurveKind, ixs::CreateMarketInstruction,
    math::ifixed_point::IFixedPoint, pda::find_borrow_position_pda, state::market_config::LtvConfig,
};

const DECIMALS_POW: u64 = 1_000_000_000; // 9 decimals => 1 unit = 1e9 atoms

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn pv(b: bool) -> &'static str {
    if b {
        "PASS"
    } else {
        "FAIL"
    }
}

/// Sign + send + wait, returning the txid string. Fails loudly on a failed tx.
async fn send_tx(
    rpc: &arch_sdk::AsyncArchRpcClient,
    network: Network,
    kp: &Keypair,
    tx: TransactionToSign,
    label: &str,
) -> Result<String> {
    let signed = tx.sign(std::slice::from_ref(kp), network);
    let txid = rpc.send_transaction(signed).await?;
    let txid_str = txid.to_string();
    let processed = rpc.wait_for_processed_transaction(&txid).await?;
    match &processed.status {
        Status::Processed => {
            println!("    [{label}] PROCESSED tx {txid_str}");
            Ok(txid_str)
        }
        other => Err(anyhow!(
            "[{label}] tx {txid_str} not processed: status={other:?} logs={:?}",
            processed.logs
        )),
    }
}

fn supply_redeemable(
    client: &AutaraFullClientWithSigner<AutaraReadClientImpl>,
    market: &Pubkey,
    holder: Keypair,
) -> u64 {
    let scoped = client.with_signer(holder);
    let Some(sp) = scoped.get_supply_position(market) else {
        return 0;
    };
    client
        .read_client()
        .get_market(market)
        .and_then(|m| m.market().supply_position_info(&sp).ok())
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<()> {
    let supply_atoms = env_u64("SL_SUPPLY_UNITS", 200_000) * DECIMALS_POW;
    let collateral_atoms = env_u64("SL_COLLATERAL_UNITS", 1) * DECIMALS_POW;
    let borrow_atoms = env_u64("SL_BORROW_UNITS", 70_000) * DECIMALS_POW;
    let donate_atoms = env_u64("SL_DONATE_UNITS", 40_000) * DECIMALS_POW;
    let collateral_price = env_f64("SL_COLLATERAL_PRICE", 100_000.0);
    let crash_price = env_f64("SL_CRASH_PRICE", 40_000.0);

    // Signing network for the stage deployment (matches the integration tests).
    let network = Network::Regtest;

    let config = ArchConfig::testnet();
    let arch_client = config.arch_rpc_client();
    println!("== SETUP: fresh market + fresh oracle feeds (stage program on testnet) ==");
    let env = AutaraTestEnv::new(
        arch_client.clone(),
        autara_stage_program_id(),
        autara_oracle_stage_program_id(),
    )
    .await
    .context("create test env (fund keypairs, mint tokens, create feeds)")?;

    // Roles.
    let curator = env.authority_keypair; // creates the market, socializes, donates
    let supplier_a = env.user_two_keypair; // the lender who eats the loss
    let borrower_b = env.user_keypair; // the bad borrower
    let curator_pk = Pubkey::from_slice(&curator.x_only_public_key().0.serialize());
    let borrower_pk = env.user_pubkey;
    let supply_mint = env.supply_mint;
    let collateral_mint = env.collateral_mint;

    // Healthy starting prices: supply asset = $1, collateral = $collateral_price.
    tokio::try_join!(
        env.push_supply_price(1.0),
        env.push_collateral_price(collateral_price)
    )
    .context("push initial prices")?;

    let program_id = env.autara_program_pubkey;
    let mut client =
        AutaraFullClientWithSigner::new_simple(arch_client.clone(), network, program_id, curator);
    client.full_reload().await?;

    // ---- create the fresh market (curator-signed) ----
    let market = client
        .with_signer(curator)
        .create_market(
            CreateMarketInstruction {
                market_bump: 0,
                index: 0,
                ltv_config: LtvConfig {
                    max_ltv: IFixedPoint::from_i64_u64_ratio(8, 10),
                    unhealthy_ltv: IFixedPoint::from_i64_u64_ratio(9, 10),
                    liquidation_bonus: IFixedPoint::from_i64_u64_ratio(5, 100),
                },
                max_utilisation_rate: IFixedPoint::from_i64_u64_ratio(9, 10),
                supply_oracle_config: env.supply_oracle_config(),
                collateral_oracle_config: env.collateral_oracle_config(),
                interest_rate: InterestRateCurveKind::new_adaptive(),
                lending_market_fee_in_bps: 100,
            },
            supply_mint,
            collateral_mint,
        )
        .await
        .context("create_market")?;
    client.full_reload().await?;
    println!("    market = {}", hex::encode(market.serialize()));
    println!("    curator (creator) = {}", hex::encode(curator_pk.serialize()));

    let position = find_borrow_position_pda(&program_id, &market, &borrower_pk).0;
    let rpc = arch_client.clone();
    let mut results: Vec<(String, bool, String)> = Vec::new();

    // ---- STEP 1: supplier A supplies liquidity ----
    println!("\n== STEP 1: supplier A supplies {supply_atoms} atoms ==");
    let tx = client.with_signer(supplier_a).tx_builder().supply(&market, supply_atoms).await?;
    let tx1 = send_tx(&rpc, network, &supplier_a, tx, "supply").await?;
    client.full_reload().await?;
    let redeemable_start = supply_redeemable(&client, &market, supplier_a);
    let pass1 = redeemable_start >= supply_atoms - 2;
    println!("    ASSERT supplier A redeemable ~= supplied -> {redeemable_start} => {}", pv(pass1));
    results.push((format!("1 supply (tx {tx1})"), pass1, format!("redeemable={redeemable_start}")));

    // ---- STEP 2: borrower B deposits collateral ----
    println!("\n== STEP 2: borrower B deposits {collateral_atoms} collateral atoms ==");
    let tx = client
        .with_signer(borrower_b)
        .tx_builder()
        .deposit_collateral(&market, collateral_atoms)
        .await?;
    let tx2 = send_tx(&rpc, network, &borrower_b, tx, "deposit").await?;
    client.full_reload().await?;
    let coll_deposited = client
        .with_signer(borrower_b)
        .get_borrow_position_health(&market)
        .map(|h| h.collateral_atoms)
        .unwrap_or(0);
    let pass2 = coll_deposited == collateral_atoms;
    println!("    ASSERT collateral_atoms == {collateral_atoms} -> {coll_deposited} => {}", pv(pass2));
    results.push((format!("2 deposit_collateral (tx {tx2})"), pass2, format!("collateral={coll_deposited}")));

    // ---- STEP 3: borrower B borrows near max LTV ----
    println!("\n== STEP 3: borrower B borrows {borrow_atoms} atoms (near max LTV) ==");
    // refresh oracle so the borrow health check reads a non-stale price
    env.push_collateral_price(collateral_price).await?;
    let tx = client.with_signer(borrower_b).tx_builder().borrow(&market, borrow_atoms).await?;
    let tx3 = send_tx(&rpc, network, &borrower_b, tx, "borrow").await?;
    client.full_reload().await?;
    let health_healthy = client.with_signer(borrower_b).get_borrow_position_health(&market)?;
    let ltv_healthy = health_healthy.ltv.to_float();
    let pass3 = health_healthy.borrowed_atoms >= borrow_atoms && ltv_healthy < 0.8;
    println!(
        "    ASSERT borrowed>=req && ltv<0.8 -> borrowed={} ltv={ltv_healthy:.4} => {}",
        health_healthy.borrowed_atoms,
        pv(pass3)
    );
    results.push((format!("3 borrow (tx {tx3})"), pass3, format!("ltv={ltv_healthy:.4}")));

    // snapshot BEFORE the crash/socialize
    let redeemable_before = supply_redeemable(&client, &market, supplier_a);
    let curator_bal_before = rpc.get_all_balances(&curator_pk).await.unwrap_or_default();
    let curator_coll_before = curator_bal_before.get(&collateral_mint).copied().unwrap_or(0);
    let curator_supply_before = curator_bal_before.get(&supply_mint).copied().unwrap_or(0);

    // ---- STEP 4: crash the collateral price so the position is underwater ----
    println!("\n== STEP 4: crash collateral price {collateral_price} -> {crash_price} ==");
    env.push_collateral_price(crash_price).await?;
    client.full_reload().await?;
    let health_crashed = client.with_signer(borrower_b).get_borrow_position_health(&market)?;
    let ltv_crashed = health_crashed.ltv.to_float();
    let pass4 = health_crashed.ltv > IFixedPoint::one();
    println!("    ASSERT ltv > 1.0 (underwater) -> ltv={ltv_crashed:.4} => {}", pv(pass4));
    results.push((format!("4 price crash"), pass4, format!("ltv={ltv_crashed:.4}")));

    // ---- STEP 5: curator socializes the loss ----
    println!("\n== STEP 5: curator calls socialize_loss ==");
    env.push_collateral_price(crash_price).await?; // keep feed fresh for the tx
    env.push_supply_price(1.0).await?;
    let tx = client.with_signer(curator).tx_builder().socialize_loss(&market, &position).await?;
    let tx5 = send_tx(&rpc, network, &curator, tx, "socialize_loss").await?;
    client.full_reload().await?;

    let redeemable_after = supply_redeemable(&client, &market, supplier_a);
    let debt_after = client
        .with_signer(borrower_b)
        .get_borrow_position_health(&market)
        .map(|h| h.borrowed_atoms)
        .unwrap_or(0);
    let curator_bal_after = rpc.get_all_balances(&curator_pk).await.unwrap_or_default();
    let curator_coll_after = curator_bal_after.get(&collateral_mint).copied().unwrap_or(0);
    let curator_supply_after = curator_bal_after.get(&supply_mint).copied().unwrap_or(0);

    let writedown = redeemable_before.saturating_sub(redeemable_after);
    let coll_swept = curator_coll_after.saturating_sub(curator_coll_before);

    // 5a: supplier A written down by ~ the socialized debt
    let pass5a = writedown >= borrow_atoms.saturating_sub(2);
    println!(
        "    ASSERT supplier A written down by ~debt -> before={redeemable_before} after={redeemable_after} writedown={writedown} (debt~{borrow_atoms}) => {}",
        pv(pass5a)
    );
    // 5b: bad position's debt cleared to zero
    let pass5b = debt_after == 0;
    println!("    ASSERT bad position debt == 0 -> {debt_after} => {}", pv(pass5b));
    // 5c: curator received ALL the collateral
    let pass5c = coll_swept == collateral_atoms;
    println!(
        "    ASSERT curator swept collateral == {collateral_atoms} -> {coll_swept} => {}",
        pv(pass5c)
    );
    // 5d: curator paid NOTHING into the pool (supply balance unchanged)
    let pass5d = curator_supply_after == curator_supply_before;
    println!(
        "    ASSERT curator supply balance unchanged (paid nothing) -> before={curator_supply_before} after={curator_supply_after} => {}",
        pv(pass5d)
    );
    results.push((format!("5 socialize_loss (tx {tx5})"), pass5a && pass5b && pass5c && pass5d, format!("writedown={writedown} debt_after={debt_after} coll_swept={coll_swept}")));

    // ---- STEP 6: curator adds back (discretionary) after off-chain sale ----
    println!("\n== STEP 6: curator donates {donate_atoms} back (simulated off-chain recovery) ==");
    let tx = client.with_signer(curator).tx_builder().donate_supply(&market, donate_atoms).await?;
    let tx6 = send_tx(&rpc, network, &curator, tx, "donate_supply").await?;
    client.full_reload().await?;
    let redeemable_recovered = supply_redeemable(&client, &market, supplier_a);
    let recovery = redeemable_recovered.saturating_sub(redeemable_after);
    // Share-price accounting rounds down: crediting `donate_atoms` over a large
    // share pool loses sub-atom dust per share, so allow a small proportional
    // tolerance (1 ppm + a floor) rather than exact equality.
    let recovery_tol = donate_atoms / 1_000_000 + 100;
    let pass6 = recovery + recovery_tol >= donate_atoms;
    println!(
        "    ASSERT supplier A recovers by ~donation -> after_socialize={redeemable_after} after_donate={redeemable_recovered} recovery={recovery} (donated={donate_atoms}) => {}",
        pv(pass6)
    );
    let net_loss = redeemable_before.saturating_sub(redeemable_recovered);
    println!("    NET supplier A loss (debt - add-back) = {net_loss} atoms");
    results.push((format!("6 donate_supply (tx {tx6})"), pass6, format!("recovery={recovery} net_loss={net_loss}")));

    // ---- summary ----
    println!("\n================ SUMMARY ================");
    let mut all = true;
    for (name, pass, detail) in &results {
        all &= pass;
        println!("  {} :: {} :: {}", pv(*pass), name, detail);
    }
    println!("=========================================");
    println!("market = {}", hex::encode(market.serialize()));
    println!("FINAL VERDICT: {}", if all { "PASS" } else { "FAIL" });
    if !all {
        std::process::exit(1);
    }
    Ok(())
}
