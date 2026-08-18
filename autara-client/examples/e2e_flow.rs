// Live testnet end-to-end lending-flow integration example for the deployed
// Autara market. Runs the full cycle (supply -> deposit collateral -> borrow
// -> repay -> withdraw collateral -> withdraw supply) and asserts position
// state after each step.
//
// Every constant below is read from an env var with the current testnet value
// as the default, so `cargo run -p autara-client --example e2e_flow` with no
// env behaves exactly as before. The CI workflow (.github/workflows/
// autara-e2e.yml) overrides the key paths via E2E_* env vars.
//
// REQUIRES a running Pyth pusher feeding the oracle program, otherwise
// borrow/repay fail with OracleRateTooOld (the submit() retry loop tolerates a
// transient staleness but not a missing pusher). Start one with:
//   cargo run --release -p autara-pyth -- \
//     --rpc https://rpc.testnet.arch.network --network testnet \
//     --program-id 8d24068aa026fd2e6ccca6e7b64a944b0e384df279b15f599ddd4a5285d592e8 \
//     --feeds 0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43,0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a
use std::time::Duration;

use anyhow::{anyhow, Result};
use arch_sdk::{
    arch_program::{
        bitcoin::key::Keypair, bitcoin::Network, pubkey::Pubkey, sanitized::ArchMessage,
    },
    build_and_sign_transaction, with_secret_key_file, AsyncArchRpcClient, Config, Status,
};
use autara_client::{
    client::{
        client_with_signer::AutaraFullClientWithSigner, read::AutaraReadClient,
        single_thread_client::AutaraReadClientImpl,
    },
    rpc_ext::ArchAsyncRpcExt,
};
use autara_lib::token::{create_ata_ix, get_associated_token_address};

// ---- env helpers: current value is the DEFAULT (no env => today's behavior) --

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("env {key}={v} is not a u64")),
        Err(_) => default,
    }
}

// deploy used BitcoinNetwork::Testnet4 for testnet signing -> default to it.
// Plain `testnet` will NOT confirm against the testnet4 cluster.
fn parse_network(s: &str) -> Network {
    match s.to_ascii_lowercase().as_str() {
        "testnet4" => Network::Testnet4,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        "mainnet" | "bitcoin" => Network::Bitcoin,
        other => panic!("unknown E2E_NETWORK={other} (use testnet4|testnet|regtest|mainnet)"),
    }
}

type Client = AutaraFullClientWithSigner<AutaraReadClientImpl>;

fn pk(h: &str) -> Pubkey {
    Pubkey::from_slice(&hex::decode(h).expect("hex"))
}

#[derive(Clone, Copy)]
enum Op {
    Supply(u64),
    Deposit(u64),
    Borrow(u64),
    Repay(Option<u64>),
    WithdrawCollateral(Option<u64>),
    WithdrawSupply(Option<u64>),
}

async fn build_tx(
    client: &Client,
    market: &Pubkey,
    op: Op,
) -> Result<autara_client::client::tx_builder::TransactionToSign> {
    let b = client.tx_builder();
    Ok(match op {
        Op::Supply(a) => b.supply(market, a).await?,
        Op::Deposit(a) => b.deposit_collateral(market, a).await?,
        Op::Borrow(a) => b.borrow(market, a).await?,
        Op::Repay(a) => b.repay(market, a).await?,
        Op::WithdrawCollateral(a) => b.withdraw_collateral(market, a).await?,
        Op::WithdrawSupply(a) => b.withdraw_supply(market, a).await?,
    })
}

/// Build + sign + send + wait. Retries once-or-twice on OracleRateTooOld.
async fn submit(
    client: &Client,
    user_kp: &Keypair,
    market: &Pubkey,
    network: Network,
    op: Op,
    label: &str,
) -> Result<String> {
    let rpc = client.rpc_client();
    for attempt in 1..=3u32 {
        let tx = build_tx(client, market, op).await?;
        let signed = tx.sign(std::slice::from_ref(user_kp), network);
        let txid = rpc.send_transaction(signed).await?;
        let txid_str = txid.to_string();
        println!("  [{label}] attempt {attempt}: sent tx {txid_str}");
        let processed = rpc.wait_for_processed_transaction(&txid).await?;
        match &processed.status {
            Status::Processed => {
                println!("  [{label}] PROCESSED tx {txid_str}");
                return Ok(txid_str);
            }
            Status::Failed(msg) => {
                let blob = format!("{msg} {:?}", processed.logs);
                if blob.contains("OracleRateTooOld") && attempt < 3 {
                    println!("  [{label}] OracleRateTooOld -> waiting 6s and retrying");
                    tokio::time::sleep(Duration::from_secs(6)).await;
                    continue;
                }
                return Err(anyhow!(
                    "[{label}] tx FAILED status={msg} logs={:?}",
                    processed.logs
                ));
            }
            Status::Queued => {
                return Err(anyhow!("[{label}] tx still QUEUED txid={txid_str}"));
            }
        }
    }
    Err(anyhow!("[{label}] exhausted retries"))
}

fn supply_owned(client: &Client, market: &Pubkey) -> u64 {
    match client.get_supply_position(market) {
        Some(sp) => client
            .read_client()
            .get_market(market)
            .and_then(|m| m.market().supply_position_info(&sp).ok())
            .unwrap_or(0),
        None => 0,
    }
}

async fn print_balances(client: &Client, user: &Pubkey, ausd: &Pubkey, abtc: &Pubkey, when: &str) {
    let bals = client
        .rpc_client()
        .get_all_balances(user)
        .await
        .unwrap_or_default();
    println!(
        "  [balances {when}] aUSD={} aBTC={}",
        bals.get(ausd).copied().unwrap_or(0),
        bals.get(abtc).copied().unwrap_or(0)
    );
}

async fn mint_to_user(
    rpc: &AsyncArchRpcClient,
    auth_path: &str,
    mint: &Pubkey,
    user: &Pubkey,
    network: Network,
    amount: u64,
    label: &str,
) -> Result<()> {
    let (auth_kp, auth_pk) =
        with_secret_key_file(auth_path).map_err(|e| anyhow!("load authority {auth_path}: {e}"))?;
    println!(
        "  [{label}] authority = {}",
        hex::encode(auth_pk.serialize())
    );
    // make sure authority can pay fees / ATA rent
    if let Err(e) = rpc.create_and_fund_account_with_faucet(&auth_kp).await {
        println!("  [{label}] authority faucet note: {e}");
    }
    let user_ata = get_associated_token_address(user, mint);
    let mut ixs = Vec::new();
    if rpc.read_account_info(user_ata).await.is_err() {
        ixs.push(create_ata_ix(&auth_pk, None, user, mint));
    }
    ixs.push(apl_token::instruction::mint_to(
        &apl_token::id(),
        mint,
        &user_ata,
        &auth_pk,
        &[],
        amount,
    )?);
    let msg = ArchMessage::new(&ixs, Some(auth_pk), rpc.get_best_block_hash().await?);
    let tx = build_and_sign_transaction(msg, vec![auth_kp], network)?;
    let txid = rpc.send_transaction(tx).await?;
    let processed = rpc.wait_for_processed_transaction(&txid).await?;
    if !matches!(processed.status, Status::Processed) {
        return Err(anyhow!(
            "[{label}] mint FAILED status={:?} logs={:?}",
            processed.status,
            processed.logs
        ));
    }
    println!("  [{label}] minted {amount} -> tx {txid}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // ---- env-driven config (defaults reproduce the original hardcoded run) ----
    let rpc_url = env_or("E2E_RPC", "https://rpc.testnet.arch.network");
    let network = parse_network(&env_or("E2E_NETWORK", "testnet4"));
    let program_id_hex = env_or(
        "E2E_PROGRAM_ID",
        "34cf72a92dd76322a42f13f99e51cf7c03221f4adbd4ee7e0c409c4161dfe20c",
    );
    let market_hex = env_or(
        "E2E_MARKET",
        "d8d679b946aafb22322f477cd5f196700f181aa3f712ca09e486fc77cedc0cce",
    );
    let ausd_hex = env_or(
        "E2E_AUSD_MINT",
        "8ec480c6e5458e7d37dc2a9f7d7d149a02d8182a38523b037905203ff36b71f6",
    );
    let abtc_hex = env_or(
        "E2E_ABTC_MINT",
        "627ecd24366c89314b12aa08a1b2fffc3890cb9cf64fb04fe3e95c7182b23dfb",
    );
    let ausd_auth = env_or(
        "E2E_AUSD_AUTHORITY",
        "/Users/brianhoffman/Projects/CLAMM/CLAMM/crates/clamm-deploy/.keys-testnet/mint-authorities/aUSD-authority.json",
    );
    let abtc_auth = env_or(
        "E2E_ABTC_AUTHORITY",
        "/Users/brianhoffman/Projects/CLAMM/CLAMM/crates/clamm-deploy/.keys-testnet/mint-authorities/aBTC-authority.json",
    );
    let user_key = env_or("E2E_USER_KEY", "/tmp/autara-e2e/user.key");

    let supply = env_u64("E2E_SUPPLY", 100_000_000);
    let collateral = env_u64("E2E_COLLATERAL", 1_000_000);
    let borrow = env_u64("E2E_BORROW", 40_000_000);
    let mint_ausd = env_u64("E2E_MINT_AUSD", 200_000_000);
    let mint_abtc = env_u64("E2E_MINT_ABTC", 2_000_000);

    let config = Config {
        arch_node_url: rpc_url,
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    };
    let rpc = AsyncArchRpcClient::new(&config);

    let program_id = pk(&program_id_hex);
    let market = pk(&market_hex);
    let ausd = pk(&ausd_hex);
    let abtc = pk(&abtc_hex);

    // ---- Step 2: create + fund test user ----
    let (user_kp, user_pk) =
        with_secret_key_file(&user_key).map_err(|e| anyhow!("user key {user_key}: {e}"))?;
    println!("== TEST USER ==");
    println!("user pubkey = {}", hex::encode(user_pk.serialize()));
    println!("user key file = {user_key}");

    // E2E_SKIP_MINT=1: mainnet mode — the user wallet is PRE-FUNDED with gas and
    // tokens (no mint authorities exist on mainnet), so skip faucet + minting.
    let skip_mint = std::env::var("E2E_SKIP_MINT")
        .map(|v| v == "1")
        .unwrap_or(false);
    if !skip_mint {
        rpc.create_and_fund_account_with_faucet(&user_kp).await?;
    }
    let acc = rpc.read_account_info(user_pk).await?;
    println!("user lamports = {}", acc.lamports);

    if !skip_mint {
        // mint generously
        mint_to_user(
            &rpc,
            &ausd_auth,
            &ausd,
            &user_pk,
            network,
            mint_ausd,
            "mint-aUSD",
        )
        .await?;
        mint_to_user(
            &rpc,
            &abtc_auth,
            &abtc,
            &user_pk,
            network,
            mint_abtc,
            "mint-aBTC",
        )
        .await?;
    } else {
        println!("E2E_SKIP_MINT=1 -> using pre-funded balances (no faucet, no mint)");
    }

    let bals = rpc.get_all_balances(&user_pk).await.unwrap_or_default();
    println!(
        "user balances: aUSD={} aBTC={}",
        bals.get(&ausd).copied().unwrap_or(0),
        bals.get(&abtc).copied().unwrap_or(0)
    );

    // ---- Step 3: flow harness ----
    let mut client =
        AutaraFullClientWithSigner::new_simple(rpc.clone(), network, program_id, user_kp);
    client.full_reload().await?;
    if client.read_client().get_market(&market).is_none() {
        return Err(anyhow!(
            "market {} not found / oracle stale after full_reload",
            market_hex
        ));
    }
    client.reload_authority_accounts_for_market(&market).await?;

    let mut results: Vec<(String, bool, String)> = Vec::new();

    // Step 1: supply
    println!("\n== STEP 1: supply {supply} aUSD ==");
    print_balances(&client, &user_pk, &ausd, &abtc, "before").await;
    let tx1 = submit(
        &client,
        &user_kp,
        &market,
        network,
        Op::Supply(supply),
        "supply",
    )
    .await;
    let tx1 = handle(tx1, &mut results, "supply")?;
    client.reload_authority_accounts_for_market(&market).await?;
    let owned = supply_owned(&client, &market);
    let pass1 = owned > 0;
    println!(
        "  ASSERT supply owned_atoms > 0 -> owned={owned} => {}",
        pv(pass1)
    );
    print_balances(&client, &user_pk, &ausd, &abtc, "after").await;
    results.push((
        format!("1 supply (tx {tx1})"),
        pass1,
        format!("owned_atoms={owned}"),
    ));

    // Step 2: deposit collateral
    println!("\n== STEP 2: deposit_collateral {collateral} aBTC ==");
    let tx2 = submit(
        &client,
        &user_kp,
        &market,
        network,
        Op::Deposit(collateral),
        "deposit",
    )
    .await;
    let tx2 = handle(tx2, &mut results, "deposit")?;
    client.reload_authority_accounts_for_market(&market).await?;
    let h2 = client.get_borrow_position_health(&market)?;
    let pass2 = h2.collateral_atoms == collateral;
    println!(
        "  ASSERT collateral_atoms == {collateral} -> {} => {}",
        h2.collateral_atoms,
        pv(pass2)
    );
    print_balances(&client, &user_pk, &ausd, &abtc, "after").await;
    results.push((
        format!("2 deposit_collateral (tx {tx2})"),
        pass2,
        format!("collateral_atoms={}", h2.collateral_atoms),
    ));

    // Step 3: borrow
    println!("\n== STEP 3: borrow {borrow} aUSD ==");
    let tx3 = submit(
        &client,
        &user_kp,
        &market,
        network,
        Op::Borrow(borrow),
        "borrow",
    )
    .await;
    let tx3 = handle(tx3, &mut results, "borrow")?;
    client.reload_authority_accounts_for_market(&market).await?;
    let h3 = client.get_borrow_position_health(&market)?;
    let ltv = h3.ltv.to_float();
    // Borrow interest accrues immediately, so the position can read a few atoms
    // above the requested amount by the time we re-read it. Tolerate that small
    // accrual rather than requiring exact equality.
    let borrow_tol = borrow / 10_000 + 10;
    let pass3 =
        h3.borrowed_atoms >= borrow && h3.borrowed_atoms <= borrow + borrow_tol && ltv < 0.8;
    println!(
        "  ASSERT {borrow} <= borrowed_atoms <= {} && ltv < 0.8 -> borrowed={} ltv={ltv:.6} => {}",
        borrow + borrow_tol,
        h3.borrowed_atoms,
        pv(pass3)
    );
    print_balances(&client, &user_pk, &ausd, &abtc, "after").await;
    results.push((
        format!("3 borrow (tx {tx3})"),
        pass3,
        format!("borrowed_atoms={} ltv={ltv:.6}", h3.borrowed_atoms),
    ));

    // Step 4: repay all.
    //
    // Repay-all must cover the debt AT EXECUTION TIME, which is the borrowed
    // amount plus the interest accrued since Step 3. A wallet holding exactly
    // the borrowed amount therefore fails with an insufficient-balance error
    // (CI only ever passed thanks to the E2E_MINT_AUSD headroom). Require a
    // small repay buffer up front so the failure is an actionable funding
    // message instead of a failed tx deep in the flow.
    println!("\n== STEP 4: repay ALL ==");
    client.reload_authority_accounts_for_market(&market).await?;
    let debt = client.get_borrow_position_health(&market)?.borrowed_atoms;
    let ausd_balance = client
        .rpc_client()
        .get_all_balances(&user_pk)
        .await
        .unwrap_or_default()
        .get(&ausd)
        .copied()
        .unwrap_or(0);
    // Same shape as borrow_tol: ~1bp of the debt (plus a floor) comfortably
    // covers the interest that accrues between this check and the repay tx.
    let repay_buffer = debt / 10_000 + 10;
    if ausd_balance < debt + repay_buffer {
        handle(
            Err(anyhow!(
                "repay-all needs a buffer above the current debt for interest accrued \
                 until the tx lands: balance={ausd_balance} aUSD < debt={debt} + buffer={repay_buffer}. \
                 Fund the user with at least {} more aUSD atoms and re-run.",
                debt + repay_buffer - ausd_balance
            )),
            &mut results,
            "repay",
        )?;
    }
    let tx4 = submit(
        &client,
        &user_kp,
        &market,
        network,
        Op::Repay(None),
        "repay",
    )
    .await;
    let tx4 = handle(tx4, &mut results, "repay")?;
    client.reload_authority_accounts_for_market(&market).await?;
    let h4 = client.get_borrow_position_health(&market)?;
    let pass4 = h4.borrowed_atoms == 0;
    println!(
        "  ASSERT borrowed_atoms == 0 -> {} => {}",
        h4.borrowed_atoms,
        pv(pass4)
    );
    print_balances(&client, &user_pk, &ausd, &abtc, "after").await;
    results.push((
        format!("4 repay (tx {tx4})"),
        pass4,
        format!("borrowed_atoms={}", h4.borrowed_atoms),
    ));

    // Step 5: withdraw collateral all
    println!("\n== STEP 5: withdraw_collateral ALL ==");
    let tx5 = submit(
        &client,
        &user_kp,
        &market,
        network,
        Op::WithdrawCollateral(None),
        "withdraw_collateral",
    )
    .await;
    let tx5 = handle(tx5, &mut results, "withdraw_collateral")?;
    client.reload_authority_accounts_for_market(&market).await?;
    let h5 = client.get_borrow_position_health(&market)?;
    let pass5 = h5.collateral_atoms == 0;
    println!(
        "  ASSERT collateral_atoms == 0 -> {} => {}",
        h5.collateral_atoms,
        pv(pass5)
    );
    print_balances(&client, &user_pk, &ausd, &abtc, "after").await;
    results.push((
        format!("5 withdraw_collateral (tx {tx5})"),
        pass5,
        format!("collateral_atoms={}", h5.collateral_atoms),
    ));

    // Step 6: withdraw supply all
    println!("\n== STEP 6: withdraw_supply ALL ==");
    let tx6 = submit(
        &client,
        &user_kp,
        &market,
        network,
        Op::WithdrawSupply(None),
        "withdraw_supply",
    )
    .await;
    let tx6 = handle(tx6, &mut results, "withdraw_supply")?;
    client.reload_authority_accounts_for_market(&market).await?;
    let owned6 = supply_owned(&client, &market);
    let pass6 = owned6 == 0;
    println!(
        "  ASSERT supply owned_atoms == 0 -> {owned6} => {}",
        pv(pass6)
    );
    print_balances(&client, &user_pk, &ausd, &abtc, "after").await;
    results.push((
        format!("6 withdraw_supply (tx {tx6})"),
        pass6,
        format!("owned_atoms={owned6}"),
    ));

    // ---- summary ----
    println!("\n================ SUMMARY ================");
    let mut all = true;
    for (name, pass, detail) in &results {
        all &= pass;
        println!("  {} :: {} :: {}", pv(*pass), name, detail);
    }
    println!("=========================================");
    println!("FINAL VERDICT: {}", if all { "PASS" } else { "FAIL" });
    if !all {
        std::process::exit(1);
    }
    Ok(())
}

fn pv(b: bool) -> &'static str {
    if b {
        "PASS"
    } else {
        "FAIL"
    }
}

// helper to unwrap submit result, recording a FAIL row on hard error then propagating
fn handle(
    r: Result<String>,
    results: &mut Vec<(String, bool, String)>,
    label: &str,
) -> Result<String> {
    match r {
        Ok(tx) => Ok(tx),
        Err(e) => {
            results.push((label.to_string(), false, format!("HARD ERROR: {e}")));
            println!("\n================ SUMMARY (aborted) ================");
            for (name, pass, detail) in results.iter() {
                println!("  {} :: {} :: {}", pv(*pass), name, detail);
            }
            Err(e)
        }
    }
}
