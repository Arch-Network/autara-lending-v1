// Live end-to-end proof of Autara's "shared loss" / loss-socialization feature.
//
// WHAT THIS PROVES (see docs/SHARED_LOSS.md for the full mechanism map):
//   1. A borrow position can be driven underwater (LTV >= 1) by an oracle price
//      crash, so a normal liquidation would leave bad debt.
//   2. The market curator (and ONLY the curator) can call `socialize_loss`,
//      which:
//        - writes down every supplier's redeemable balance pro-rata by the
//          full outstanding debt (the pool eats the loss). This is a SHARE-
//          PRICE WRITEDOWN inside the market account, NOT a token transfer,
//        - clears the bad position's debt and collateral to zero,
//        - transfers ALL of the position's collateral to the CURATOR's wallet
//          (the ONLY token transfer in the instruction) WITHOUT the curator
//          paying anything into the pool.
//   3. The curator can later (at its sole discretion, any amount) call
//      `donate_supply` to add value back to the pool (the second visible token
//      transfer), which raises every supplier's redeemable balance pro-rata.
//      Nothing on-chain forces the curator to add anything back, or to add
//      back a "fair" amount.
//
// This runs on a FRESH, ISOLATED market with FRESH FAKE oracle feeds. It NEVER
// touches the shared aUSD/aBTC market or the real BTC/USDC feeds (hard guards
// below refuse both).
//
// TARGET (defaults reproduce the original *stage* run):
//     SL_PROGRAM_ID   lending program id (hex)         [default: stage program]
//     SL_ORACLE_ID    oracle program id (hex)          [default: stage oracle]
//     SL_CURATOR_KEY  path to a secret-key file; this signer creates the market
//                     and calls socialize_loss/donate  [default: harness authority]
//     SL_NETWORK      signing network testnet4|testnet|regtest|mainnet [default: regtest]
//     SL_RPC          arch rpc url                     [default: testnet rpc]
//     SL_VERIFY_ONLY  =1 -> run only the permissionless-push pre-flight and exit
//
// TOKENS: by default the harness creates fresh 9-decimal mints. To prove with
// EXISTING mints (e.g. the real aUSD/aBTC), set ALL of:
//     SL_SUPPLY_MINT               existing supply mint (hex)
//     SL_COLLATERAL_MINT           existing collateral mint (hex)
//     SL_SUPPLY_MINT_AUTHORITY     path to the supply mint-authority key file
//     SL_COLLATERAL_MINT_AUTHORITY path to the collateral mint-authority key file
//   and optionally pin the FAKE feed ids (fresh random by default):
//     SL_SUPPLY_FEED / SL_COLLATERAL_FEED  32-byte hex feed ids
//   Decimals are read from the mint accounts on-chain, so unit knobs below keep
//   their meaning (1 unit = 10^decimals atoms).
//
// Run (stage, unchanged):
//   cargo run -p autara-client --example shared_loss_flow
//
// Amount knobs (all optional; defaults give a realistic 70% LTV loan hit by a
// 60% collateral crash):
//   SL_SUPPLY_UNITS      supplier A deposit, whole supply units   (default 200000)
//   SL_COLLATERAL_UNITS  borrower B collateral, whole units       (default 1)
//   SL_BORROW_UNITS      borrower B borrow, whole supply units    (default 70000)
//   SL_COLLATERAL_PRICE  healthy collateral price in USD          (default 100000)
//   SL_CRASH_PRICE       crashed collateral price in USD          (default 40000)
//   SL_DONATE_UNITS      curator add-back after off-chain sale    (default 40000)

use anyhow::{anyhow, Context, Result};
use arch_sdk::{
    arch_program::{bitcoin::key::Keypair, bitcoin::Network, pubkey::Pubkey},
    generate_new_keypair, with_secret_key_file, AsyncArchRpcClient, Config, Status,
};
use autara_client::{
    client::{
        client_with_signer::AutaraFullClientWithSigner, read::AutaraReadClient,
        single_thread_client::AutaraReadClientImpl, tx_builder::TransactionToSign,
    },
    config::{autara_oracle_stage_program_id, autara_stage_program_id, ArchConfig},
    rpc_ext::ArchAsyncRpcExt,
    test::{AutaraTestEnv, TokenMinter},
};
use autara_lib::{
    interest_rate::interest_rate_kind::InterestRateCurveKind,
    ixs::CreateMarketInstruction,
    math::ifixed_point::IFixedPoint,
    oracle::{oracle_config::OracleConfig, pyth::PythPrice},
    pda::{find_borrow_position_pda, find_market_pda},
    state::market_config::LtvConfig,
};
use autara_pyth::{get_pyth_account, AutaraPythPusherClient};

// SAFETY: real BTC/USDC feed ids that MUST NEVER be pushed by this proof.
const REAL_BTC_FEED: &str = "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43";
const REAL_USDC_FEED: &str = "eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a";
// SAFETY: the shared aUSD/aBTC market that MUST NEVER be touched.
const SHARED_MARKET: &str = "d8d679b946aafb22322f477cd5f196700f181aa3f712ca09e486fc77cedc0cce";

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

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn parse_network(s: &str) -> Network {
    match s.to_ascii_lowercase().as_str() {
        "testnet4" => Network::Testnet4,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        "mainnet" | "bitcoin" => Network::Bitcoin,
        other => panic!("unknown SL_NETWORK={other} (use testnet4|testnet|regtest|mainnet)"),
    }
}

fn pk_from_hex(h: &str) -> Pubkey {
    Pubkey::from_slice(&hex::decode(h).expect("valid hex pubkey"))
}

fn feed_from_hex(h: &str) -> [u8; 32] {
    let bytes = hex::decode(h).expect("valid hex feed id");
    bytes
        .as_slice()
        .try_into()
        .expect("feed id must be 32 bytes")
}

fn assert_fake_feed(feed_id: [u8; 32], label: &str) {
    let h = hex::encode(feed_id);
    assert!(
        h != REAL_BTC_FEED && h != REAL_USDC_FEED,
        "{label} feed id {h} is a REAL feed id -- refusing to push to it"
    );
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

/// Pre-flight: confirm that price pushes to this oracle are PERMISSIONLESS by
/// creating a FRESH fake feed (random 32-byte id) and pushing to it with an
/// arbitrary, unrelated signer. Reports the on-chain feed account size (legacy
/// pre-authority feeds are exactly one `PythPrice` = 120 bytes). Returns the
/// feed account data length on success.
async fn verify_permissionless_push(
    rpc: &AsyncArchRpcClient,
    oracle_id: &Pubkey,
    network: Network,
) -> Result<usize> {
    println!("== PRE-FLIGHT: verify permissionless push + feed layout ==");
    // Arbitrary signer unrelated to curator/admin.
    let (signer, signer_pk, _) = generate_new_keypair(network);
    rpc.create_and_fund_account_with_faucet(&signer)
        .await
        .context("faucet-fund arbitrary push signer")?;
    // Fresh random fake feed id (a brand new keypair's pubkey bytes).
    let (_k, feed_pk, _) = generate_new_keypair(network);
    let feed_id = feed_pk.0;
    assert_fake_feed(feed_id, "pre-flight");
    println!(
        "    arbitrary signer = {}",
        hex::encode(signer_pk.serialize())
    );
    println!("    fresh fake feed id = {}", hex::encode(feed_id));

    let price = PythPrice::from_dummy(feed_id, 123.45);
    AutaraPythPusherClient {
        client: rpc.clone(),
        autara_oracle_program_id: *oracle_id,
        network,
    }
    .push_pyth_price(&signer, feed_id, &price)
    .await
    .context("push to fresh fake feed with arbitrary signer")?;

    let pyth_account = get_pyth_account(oracle_id, feed_id);
    let acc = rpc
        .read_account_info(pyth_account)
        .await
        .context("read back fresh feed account")?;
    let owner_ok = acc.owner == *oracle_id;
    let len = acc.data.len();
    let legacy_120 = len == core::mem::size_of::<PythPrice>();
    // decode the price (legacy layout is exactly one PythPrice at the front)
    let stored: Option<&PythPrice> = bytemuck::try_from_bytes::<PythPrice>(
        &acc.data[..core::mem::size_of::<PythPrice>().min(len)],
    )
    .ok();
    let id_ok = stored.map(|p| p.id == feed_id).unwrap_or(false);
    println!(
        "    feed account = {} owner_ok={owner_ok} size={len} bytes (legacy120={legacy_120}) id_ok={id_ok}",
        hex::encode(pyth_account.serialize())
    );
    if !(owner_ok && id_ok) {
        return Err(anyhow!(
            "permissionless push verification FAILED (owner_ok={owner_ok} id_ok={id_ok}); \
             the oracle may be authority-gated -- STOP and escalate (redeploy is a human decision)"
        ));
    }
    println!("    => PERMISSIONLESS PUSH CONFIRMED");
    Ok(len)
}

/// Everything the flow needs, whether the tokens are harness-created fresh
/// mints (default / stage behavior) or pre-existing mints (e.g. real aUSD/aBTC)
/// minted to the test users via their real mint authorities.
struct Harness {
    rpc: AsyncArchRpcClient,
    network: Network,
    oracle_id: Pubkey,
    /// signer used for every fake-feed price push
    pusher: Keypair,
    supply_feed_id: [u8; 32],
    collateral_feed_id: [u8; 32],
    supply_mint: Pubkey,
    collateral_mint: Pubkey,
    supply_minter: TokenMinter,
    supplier_a: Keypair,
    borrower_b: Keypair,
    borrower_pk: Pubkey,
    /// curator used when SL_CURATOR_KEY is not set
    default_curator: Keypair,
}

impl Harness {
    async fn push(&self, feed_id: [u8; 32], price: f64) -> Result<()> {
        assert_fake_feed(feed_id, "push");
        let pyth = PythPrice::from_dummy(feed_id, price);
        AutaraPythPusherClient {
            client: self.rpc.clone(),
            autara_oracle_program_id: self.oracle_id,
            network: self.network,
        }
        .push_pyth_price(&self.pusher, feed_id, &pyth)
        .await
    }

    async fn push_supply_price(&self, price: f64) -> Result<()> {
        self.push(self.supply_feed_id, price).await
    }

    async fn push_collateral_price(&self, price: f64) -> Result<()> {
        self.push(self.collateral_feed_id, price).await
    }

    fn supply_oracle_config(&self) -> OracleConfig {
        OracleConfig::new_pyth(self.supply_feed_id, self.oracle_id)
    }

    fn collateral_oracle_config(&self) -> OracleConfig {
        OracleConfig::new_pyth(self.collateral_feed_id, self.oracle_id)
    }
}

/// Existing-mints path: fund fresh users, mint the pre-existing tokens to them
/// with the supplied mint authorities, and pick fresh fake feed ids.
async fn harness_from_existing_mints(
    rpc: &AsyncArchRpcClient,
    network: Network,
    oracle_id: Pubkey,
    supply_mint: Pubkey,
    collateral_mint: Pubkey,
    supply_atoms: u64,
    collateral_atoms: u64,
) -> Result<Harness> {
    let supply_auth_path = std::env::var("SL_SUPPLY_MINT_AUTHORITY")
        .map_err(|_| anyhow!("SL_SUPPLY_MINT set but SL_SUPPLY_MINT_AUTHORITY missing"))?;
    let collateral_auth_path = std::env::var("SL_COLLATERAL_MINT_AUTHORITY")
        .map_err(|_| anyhow!("SL_COLLATERAL_MINT set but SL_COLLATERAL_MINT_AUTHORITY missing"))?;
    let (supply_auth, supply_auth_pk) = with_secret_key_file(&supply_auth_path)
        .map_err(|e| anyhow!("load {supply_auth_path}: {e}"))?;
    let (collateral_auth, collateral_auth_pk) = with_secret_key_file(&collateral_auth_path)
        .map_err(|e| anyhow!("load {collateral_auth_path}: {e}"))?;
    println!(
        "    supply mint authority = {}",
        hex::encode(supply_auth_pk.serialize())
    );
    println!(
        "    collateral mint authority = {}",
        hex::encode(collateral_auth_pk.serialize())
    );

    let (supplier_a, _a_pk, _) = generate_new_keypair(network);
    let (borrower_b, borrower_pk, _) = generate_new_keypair(network);
    let (pusher, _p_pk, _) = generate_new_keypair(network);
    tokio::try_join!(
        rpc.create_and_fund_account_with_faucet(&supplier_a),
        rpc.create_and_fund_account_with_faucet(&borrower_b),
        rpc.create_and_fund_account_with_faucet(&pusher),
        rpc.create_and_fund_account_with_faucet(&supply_auth),
        rpc.create_and_fund_account_with_faucet(&collateral_auth),
    )
    .context("faucet-fund users/pusher/mint authorities")?;

    // Fresh fake feed ids (env-pinnable), guaranteed != real feeds.
    let supply_feed_id = std::env::var("SL_SUPPLY_FEED")
        .map(|h| feed_from_hex(&h))
        .unwrap_or_else(|_| generate_new_keypair(network).1 .0);
    let collateral_feed_id = std::env::var("SL_COLLATERAL_FEED")
        .map(|h| feed_from_hex(&h))
        .unwrap_or_else(|_| generate_new_keypair(network).1 .0);
    assert_fake_feed(supply_feed_id, "supply");
    assert_fake_feed(collateral_feed_id, "collateral");

    let supply_minter = TokenMinter::from_existing(rpc.clone(), supply_auth, supply_mint, network);
    let collateral_minter =
        TokenMinter::from_existing(rpc.clone(), collateral_auth, collateral_mint, network);

    let a_pk = Pubkey::from_slice(&supplier_a.x_only_public_key().0.serialize());
    supply_minter
        .mint_to(&a_pk, supply_atoms)
        .await
        .context("mint supply tokens to supplier A")?;
    collateral_minter
        .mint_to(&borrower_pk, collateral_atoms)
        .await
        .context("mint collateral tokens to borrower B")?;

    Ok(Harness {
        rpc: rpc.clone(),
        network,
        oracle_id,
        pusher,
        supply_feed_id,
        collateral_feed_id,
        supply_mint,
        collateral_mint,
        supply_minter,
        supplier_a,
        borrower_b,
        borrower_pk,
        default_curator: pusher,
    })
}

/// A point-in-time view of every balance the proof narrates: real on-chain
/// token balances for the three parties plus supplier A's REDEEMABLE value
/// (internal market share-price state; NOT a token balance).
#[derive(Clone, Copy, Default)]
struct Snapshot {
    a_supply: u64,
    a_coll: u64,
    b_supply: u64,
    b_coll: u64,
    cur_supply: u64,
    cur_coll: u64,
    redeemable: u64,
}

async fn snapshot(
    rpc: &AsyncArchRpcClient,
    client: &AutaraFullClientWithSigner<AutaraReadClientImpl>,
    market: Option<&Pubkey>,
    supplier_a: Keypair,
    a_pk: &Pubkey,
    b_pk: &Pubkey,
    cur_pk: &Pubkey,
    supply_mint: &Pubkey,
    collateral_mint: &Pubkey,
) -> Snapshot {
    let bal = |pk: &Pubkey| {
        let rpc = rpc.clone();
        let pk = *pk;
        async move { rpc.get_all_balances(&pk).await.unwrap_or_default() }
    };
    let (a, b, c) = tokio::join!(bal(a_pk), bal(b_pk), bal(cur_pk));
    Snapshot {
        a_supply: a.get(supply_mint).copied().unwrap_or(0),
        a_coll: a.get(collateral_mint).copied().unwrap_or(0),
        b_supply: b.get(supply_mint).copied().unwrap_or(0),
        b_coll: b.get(collateral_mint).copied().unwrap_or(0),
        cur_supply: c.get(supply_mint).copied().unwrap_or(0),
        cur_coll: c.get(collateral_mint).copied().unwrap_or(0),
        redeemable: market
            .map(|m| supply_redeemable(client, m, supplier_a))
            .unwrap_or(0),
    }
}

fn d(before: u64, after: u64) -> String {
    match after.cmp(&before) {
        std::cmp::Ordering::Greater => format!("+{}", after - before),
        std::cmp::Ordering::Less => format!("-{}", before - after),
        std::cmp::Ordering::Equal => "0".to_string(),
    }
}

fn print_snapshot(label: &str, prev: &Snapshot, now: &Snapshot) {
    println!("    [balances {label}] (token balances = real on-chain transfers; redeemable = internal share-price state)");
    println!(
        "      supplier A : supply={} ({})  collateral={} ({})",
        now.a_supply,
        d(prev.a_supply, now.a_supply),
        now.a_coll,
        d(prev.a_coll, now.a_coll)
    );
    println!(
        "      borrower B : supply={} ({})  collateral={} ({})",
        now.b_supply,
        d(prev.b_supply, now.b_supply),
        now.b_coll,
        d(prev.b_coll, now.b_coll)
    );
    println!(
        "      curator    : supply={} ({})  collateral={} ({})",
        now.cur_supply,
        d(prev.cur_supply, now.cur_supply),
        now.cur_coll,
        d(prev.cur_coll, now.cur_coll)
    );
    println!(
        "      supplier A REDEEMABLE (not a token transfer) = {} ({})",
        now.redeemable,
        d(prev.redeemable, now.redeemable)
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    // Target selection (defaults reproduce the stage run).
    let network = parse_network(&env_or("SL_NETWORK", "regtest"));
    let program_id = std::env::var("SL_PROGRAM_ID")
        .map(|h| pk_from_hex(&h))
        .unwrap_or_else(|_| autara_stage_program_id());
    let oracle_id = std::env::var("SL_ORACLE_ID")
        .map(|h| pk_from_hex(&h))
        .unwrap_or_else(|_| autara_oracle_stage_program_id());

    // Build the rpc client honoring SL_NETWORK so faucet funding matches the
    // network used to sign every other transaction.
    let base = ArchConfig::testnet();
    let config = Config {
        arch_node_url: env_or("SL_RPC", &base.arch_node_url),
        node_endpoint: base.bitcoin_node_endpoint.clone(),
        node_username: base.bitcoin_node_username.clone(),
        node_password: base.bitcoin_node_password.clone(),
        network,
        titan_url: String::new(),
    };
    let arch_client = AsyncArchRpcClient::new(&config);

    println!("== TARGET ==");
    println!("    program = {}", hex::encode(program_id.serialize()));
    println!("    oracle  = {}", hex::encode(oracle_id.serialize()));
    println!("    network = {network:?}");
    println!("    rpc     = {}", config.arch_node_url);

    // ---- PRE-FLIGHT: permissionless-push verification (always) ----
    let feed_size = verify_permissionless_push(&arch_client, &oracle_id, network).await?;

    if env_or("SL_VERIFY_ONLY", "0") == "1" {
        println!("\nSL_VERIFY_ONLY=1 -> feed_size={feed_size}; skipping the full flow.");
        return Ok(());
    }

    // ---- token/mint selection ----
    let existing_mints = match (
        std::env::var("SL_SUPPLY_MINT"),
        std::env::var("SL_COLLATERAL_MINT"),
    ) {
        (Ok(s), Ok(c)) => Some((pk_from_hex(&s), pk_from_hex(&c))),
        (Err(_), Err(_)) => None,
        _ => {
            return Err(anyhow!(
                "set BOTH SL_SUPPLY_MINT and SL_COLLATERAL_MINT (or neither)"
            ))
        }
    };

    // Unit knobs; atoms are computed after we know each mint's decimals.
    let supply_units = env_u64("SL_SUPPLY_UNITS", 200_000);
    let collateral_units = env_u64("SL_COLLATERAL_UNITS", 1);
    let borrow_units = env_u64("SL_BORROW_UNITS", 70_000);
    let donate_units = env_u64("SL_DONATE_UNITS", 40_000);
    let collateral_price = env_f64("SL_COLLATERAL_PRICE", 100_000.0);
    let crash_price = env_f64("SL_CRASH_PRICE", 40_000.0);

    println!("\n== SETUP: fresh isolated market + fresh FAKE oracle feeds ==");
    let harness = match existing_mints {
        Some((supply_mint, collateral_mint)) => {
            println!("    using EXISTING mints (e.g. real aUSD/aBTC)");
            // need decimals before minting the right amounts
            let mints = arch_client
                .get_mints(&[supply_mint, collateral_mint])
                .await
                .context("fetch existing mint accounts")?;
            let sd = mints
                .get(&supply_mint)
                .context("supply mint not found on-chain")?
                .decimals() as u32;
            let cd = mints
                .get(&collateral_mint)
                .context("collateral mint not found on-chain")?
                .decimals() as u32;
            println!("    supply decimals = {sd}, collateral decimals = {cd}");
            harness_from_existing_mints(
                &arch_client,
                network,
                oracle_id,
                supply_mint,
                collateral_mint,
                supply_units * 10u64.pow(sd),
                collateral_units * 10u64.pow(cd),
            )
            .await?
        }
        None => {
            println!("    using fresh harness mints (stage behavior)");
            let env = AutaraTestEnv::new_with_network(
                arch_client.clone(),
                program_id,
                oracle_id,
                network,
            )
            .await
            .context("create test env (fund keypairs, mint tokens, create feeds)")?;
            Harness {
                rpc: arch_client.clone(),
                network,
                oracle_id,
                pusher: env.authority_keypair,
                supply_feed_id: env.supply_feed_id,
                collateral_feed_id: env.collateral_feed_id,
                supply_mint: env.supply_mint,
                collateral_mint: env.collateral_mint,
                supply_minter: env.supply_minter.clone(),
                supplier_a: env.user_two_keypair,
                borrower_b: env.user_keypair,
                borrower_pk: env.user_pubkey,
                default_curator: env.authority_keypair,
            }
        }
    };

    let supply_mint = harness.supply_mint;
    let collateral_mint = harness.collateral_mint;

    // Now that mints exist, resolve decimals -> atoms (harness mints are 9 dec).
    let mints = arch_client
        .get_mints(&[supply_mint, collateral_mint])
        .await
        .context("fetch mint accounts for decimals")?;
    let supply_dec = mints.get(&supply_mint).context("supply mint")?.decimals() as u32;
    let collateral_dec = mints
        .get(&collateral_mint)
        .context("collateral mint")?
        .decimals() as u32;
    let supply_atoms = supply_units * 10u64.pow(supply_dec);
    let collateral_atoms = collateral_units * 10u64.pow(collateral_dec);
    let borrow_atoms = borrow_units * 10u64.pow(supply_dec);
    let donate_atoms = donate_units * 10u64.pow(supply_dec);

    // Roles. The curator creates the market, socializes, and donates.
    let (curator, curator_overridden) = match std::env::var("SL_CURATOR_KEY") {
        Ok(path) => {
            let (kp, _pk) = with_secret_key_file(&path)
                .map_err(|e| anyhow!("load SL_CURATOR_KEY {path}: {e}"))?;
            (kp, true)
        }
        Err(_) => (harness.default_curator, false),
    };
    let supplier_a = harness.supplier_a; // the lender who eats the loss
    let borrower_b = harness.borrower_b; // the bad borrower
    let curator_pk = Pubkey::from_slice(&curator.x_only_public_key().0.serialize());
    let supplier_a_pk = Pubkey::from_slice(&supplier_a.x_only_public_key().0.serialize());
    let borrower_pk = harness.borrower_pk;

    // SAFETY guards: never operate on the shared market or push real feeds.
    assert_fake_feed(harness.supply_feed_id, "market supply");
    assert_fake_feed(harness.collateral_feed_id, "market collateral");

    // Fund the curator with gas and enough supply tokens for the donate step.
    if curator_overridden {
        if let Err(e) = arch_client
            .create_and_fund_account_with_faucet(&curator)
            .await
        {
            println!("    curator faucet note: {e}");
        }
    }
    let cur_supply_balance = arch_client
        .get_all_balances(&curator_pk)
        .await
        .unwrap_or_default()
        .get(&supply_mint)
        .copied()
        .unwrap_or(0);
    if cur_supply_balance < donate_atoms {
        harness
            .supply_minter
            .mint_to(&curator_pk, donate_atoms.saturating_mul(2))
            .await
            .context("mint supply tokens to curator for donate_supply")?;
    }

    // Healthy starting prices: supply asset = $1, collateral = $collateral_price.
    tokio::try_join!(
        harness.push_supply_price(1.0),
        harness.push_collateral_price(collateral_price)
    )
    .context("push initial prices")?;

    let mut client =
        AutaraFullClientWithSigner::new_simple(arch_client.clone(), network, program_id, curator);
    client.full_reload().await?;

    // ---- pick a market index that can NEVER be the shared market ----
    // The market PDA is seeded by (curator, supply_mint, collateral_mint,
    // index). With the real curator + real mints an index may collide with the
    // shared aUSD/aBTC market, so scan for the first index whose PDA is not the
    // shared market and does not exist yet.
    let mut market_index = None;
    for idx in 0u8..=255 {
        let (pda, _) = find_market_pda(
            &program_id,
            &curator_pk,
            &supply_mint,
            &collateral_mint,
            idx,
        );
        if hex::encode(pda.serialize()) == SHARED_MARKET {
            println!("    index {idx} would be the SHARED market -> skipping (guard held)");
            continue;
        }
        if arch_client.read_account_info(pda).await.is_ok() {
            println!(
                "    index {idx} already has a market ({}) -> skipping",
                hex::encode(pda.serialize())
            );
            continue;
        }
        market_index = Some(idx);
        break;
    }
    let market_index = market_index.context("no free market index")?;
    println!("    using market index {market_index}");

    // ---- create the fresh market (curator-signed) ----
    let market = client
        .with_signer(curator)
        .create_market(
            CreateMarketInstruction {
                market_bump: 0,
                index: market_index,
                ltv_config: LtvConfig {
                    max_ltv: IFixedPoint::from_i64_u64_ratio(8, 10),
                    unhealthy_ltv: IFixedPoint::from_i64_u64_ratio(9, 10),
                    liquidation_bonus: IFixedPoint::from_i64_u64_ratio(5, 100),
                },
                max_utilisation_rate: IFixedPoint::from_i64_u64_ratio(9, 10),
                supply_oracle_config: harness.supply_oracle_config(),
                collateral_oracle_config: harness.collateral_oracle_config(),
                interest_rate: InterestRateCurveKind::new_adaptive(),
                lending_market_fee_in_bps: 100,
            },
            supply_mint,
            collateral_mint,
        )
        .await
        .context("create_market")?;
    client.full_reload().await?;
    // Final safety assertion: the market we created is NOT the shared market.
    assert!(
        hex::encode(market.serialize()) != SHARED_MARKET,
        "refusing to operate on the shared aUSD/aBTC market"
    );
    println!("    market = {}", hex::encode(market.serialize()));
    println!(
        "    curator (creator) = {}",
        hex::encode(curator_pk.serialize())
    );
    println!(
        "    supplier A = {}",
        hex::encode(supplier_a_pk.serialize())
    );
    println!("    borrower B = {}", hex::encode(borrower_pk.serialize()));
    println!("    supply_mint = {}", hex::encode(supply_mint.serialize()));
    println!(
        "    collateral_mint = {}",
        hex::encode(collateral_mint.serialize())
    );
    println!(
        "    supply_feed (FAKE) = {}",
        hex::encode(harness.supply_feed_id)
    );
    println!(
        "    collateral_feed (FAKE) = {}",
        hex::encode(harness.collateral_feed_id)
    );

    let position = find_borrow_position_pda(&program_id, &market, &borrower_pk).0;
    let rpc = arch_client.clone();
    let mut results: Vec<(String, bool, String)> = Vec::new();

    macro_rules! snap {
        () => {
            snapshot(
                &rpc,
                &client,
                Some(&market),
                supplier_a,
                &supplier_a_pk,
                &borrower_pk,
                &curator_pk,
                &supply_mint,
                &collateral_mint,
            )
        };
    }

    let baseline = snap!().await;
    print_snapshot("baseline", &baseline, &baseline);

    // ---- STEP 1: supplier A supplies liquidity ----
    println!("\n== STEP 1: supplier A supplies {supply_atoms} supply atoms ==");
    println!("    (token transfer: A's supply tokens -> market vault)");
    let tx = client
        .with_signer(supplier_a)
        .tx_builder()
        .supply(&market, supply_atoms)
        .await?;
    let tx1 = send_tx(&rpc, network, &supplier_a, tx, "supply").await?;
    client.full_reload().await?;
    let s1 = snap!().await;
    print_snapshot("after step 1", &baseline, &s1);
    let pass1 = s1.redeemable >= supply_atoms - 2;
    println!(
        "    ASSERT supplier A redeemable ~= supplied -> {} => {}",
        s1.redeemable,
        pv(pass1)
    );
    results.push((
        format!("1 supply (tx {tx1})"),
        pass1,
        format!("redeemable={}", s1.redeemable),
    ));

    // ---- STEP 2: borrower B deposits collateral ----
    println!("\n== STEP 2: borrower B deposits {collateral_atoms} collateral atoms ==");
    println!("    (token transfer: B's collateral tokens -> market vault)");
    let tx = client
        .with_signer(borrower_b)
        .tx_builder()
        .deposit_collateral(&market, collateral_atoms)
        .await?;
    let tx2 = send_tx(&rpc, network, &borrower_b, tx, "deposit").await?;
    client.full_reload().await?;
    let s2 = snap!().await;
    print_snapshot("after step 2", &s1, &s2);
    let coll_deposited = client
        .with_signer(borrower_b)
        .get_borrow_position_health(&market)
        .map(|h| h.collateral_atoms)
        .unwrap_or(0);
    let pass2 = coll_deposited == collateral_atoms;
    println!(
        "    ASSERT collateral_atoms == {collateral_atoms} -> {coll_deposited} => {}",
        pv(pass2)
    );
    results.push((
        format!("2 deposit_collateral (tx {tx2})"),
        pass2,
        format!("collateral={coll_deposited}"),
    ));

    // ---- STEP 3: borrower B borrows near max LTV ----
    println!("\n== STEP 3: borrower B borrows {borrow_atoms} supply atoms (near max LTV) ==");
    println!("    (token transfer: market vault supply tokens -> B)");
    // refresh oracle so the borrow health check reads a non-stale price
    harness.push_collateral_price(collateral_price).await?;
    let tx = client
        .with_signer(borrower_b)
        .tx_builder()
        .borrow(&market, borrow_atoms)
        .await?;
    let tx3 = send_tx(&rpc, network, &borrower_b, tx, "borrow").await?;
    client.full_reload().await?;
    let s3 = snap!().await;
    print_snapshot("after step 3", &s2, &s3);
    let health_healthy = client
        .with_signer(borrower_b)
        .get_borrow_position_health(&market)?;
    let ltv_healthy = health_healthy.ltv.to_float();
    let pass3 = health_healthy.borrowed_atoms >= borrow_atoms && ltv_healthy < 0.8;
    println!(
        "    ASSERT borrowed>=req && ltv<0.8 -> borrowed={} ltv={ltv_healthy:.4} => {}",
        health_healthy.borrowed_atoms,
        pv(pass3)
    );
    results.push((
        format!("3 borrow (tx {tx3})"),
        pass3,
        format!("ltv={ltv_healthy:.4}"),
    ));

    // ---- STEP 4: crash the collateral price so the position is underwater ----
    println!("\n== STEP 4: crash FAKE collateral feed {collateral_price} -> {crash_price} ==");
    println!("    (no token transfer: oracle push only; real feeds untouched)");
    harness.push_collateral_price(crash_price).await?;
    client.full_reload().await?;
    let s4 = snap!().await;
    print_snapshot("after step 4", &s3, &s4);
    let health_crashed = client
        .with_signer(borrower_b)
        .get_borrow_position_health(&market)?;
    let ltv_crashed = health_crashed.ltv.to_float();
    let pass4 = health_crashed.ltv > IFixedPoint::one();
    println!(
        "    ASSERT ltv > 1.0 (underwater) -> ltv={ltv_crashed:.4} => {}",
        pv(pass4)
    );
    results.push((
        "4 price crash".to_string(),
        pass4,
        format!("ltv={ltv_crashed:.4}"),
    ));

    // ---- STEP 5a: NON-curator socialize_loss MUST be rejected ----
    println!("\n== STEP 5a: non-curator socialize_loss must be REJECTED ==");
    harness.push_collateral_price(crash_price).await?;
    harness.push_supply_price(1.0).await?;
    let non_curator_rejected = match client
        .with_signer(borrower_b)
        .tx_builder()
        .socialize_loss(&market, &position)
        .await
    {
        Ok(tx) => {
            // Built ok; sending it must fail on-chain (curator check).
            let signed = tx.sign(std::slice::from_ref(&borrower_b), network);
            match rpc.send_transaction(signed).await {
                Ok(txid) => {
                    let processed = rpc.wait_for_processed_transaction(&txid).await?;
                    !matches!(processed.status, Status::Processed)
                }
                Err(_) => true,
            }
        }
        Err(_) => true,
    };
    println!(
        "    ASSERT non-curator socialize_loss rejected -> {}",
        pv(non_curator_rejected)
    );
    results.push((
        "5a non-curator socialize_loss rejected".to_string(),
        non_curator_rejected,
        format!("rejected={non_curator_rejected}"),
    ));

    // ---- STEP 5b: curator socializes the loss ----
    println!("\n== STEP 5b: curator calls socialize_loss ==");
    println!("    (ONE token transfer: market vault collateral -> curator ATA.");
    println!("     The supplier loss is a share-price WRITEDOWN inside the market");
    println!("     account -- watch REDEEMABLE drop with NO supply-token movement.)");
    harness.push_collateral_price(crash_price).await?; // keep feed fresh for the tx
    harness.push_supply_price(1.0).await?;
    let s_before = snap!().await;
    let tx = client
        .with_signer(curator)
        .tx_builder()
        .socialize_loss(&market, &position)
        .await?;
    let tx5 = send_tx(&rpc, network, &curator, tx, "socialize_loss").await?;
    client.full_reload().await?;
    let s5 = snap!().await;
    print_snapshot("after step 5b", &s_before, &s5);

    let debt_after = client
        .with_signer(borrower_b)
        .get_borrow_position_health(&market)
        .map(|h| h.borrowed_atoms)
        .unwrap_or(0);
    let writedown = s_before.redeemable.saturating_sub(s5.redeemable);
    let coll_swept = s5.cur_coll.saturating_sub(s_before.cur_coll);

    // 5b-i: supplier A written down by ~ the socialized debt
    let pass5a = writedown >= borrow_atoms.saturating_sub(2);
    println!(
        "    ASSERT supplier A REDEEMABLE written down by ~debt -> before={} after={} writedown={writedown} (debt~{borrow_atoms}) => {}",
        s_before.redeemable,
        s5.redeemable,
        pv(pass5a)
    );
    // 5b-ii: bad position's debt cleared to zero
    let pass5b = debt_after == 0;
    println!(
        "    ASSERT bad position debt == 0 -> {debt_after} => {}",
        pv(pass5b)
    );
    // 5b-iii: curator received ALL the collateral (the ONE visible token transfer)
    let pass5c = coll_swept == collateral_atoms;
    println!(
        "    ASSERT curator swept collateral == {collateral_atoms} -> {coll_swept} => {}",
        pv(pass5c)
    );
    // 5b-iv: curator paid NOTHING into the pool (supply balance unchanged)
    let pass5d = s5.cur_supply == s_before.cur_supply;
    println!(
        "    ASSERT curator supply balance unchanged (paid nothing) -> before={} after={} => {}",
        s_before.cur_supply,
        s5.cur_supply,
        pv(pass5d)
    );
    // 5b-v: supplier A's TOKEN balance did not move (the loss is not a transfer)
    let pass5e = s5.a_supply == s_before.a_supply;
    println!(
        "    ASSERT supplier A supply TOKEN balance unchanged (loss is a writedown, not a transfer) -> before={} after={} => {}",
        s_before.a_supply,
        s5.a_supply,
        pv(pass5e)
    );
    results.push((
        format!("5b socialize_loss (tx {tx5})"),
        pass5a && pass5b && pass5c && pass5d && pass5e,
        format!("writedown={writedown} debt_after={debt_after} coll_swept={coll_swept}"),
    ));

    // ---- STEP 6: curator adds back (discretionary) after off-chain sale ----
    println!("\n== STEP 6: curator donates {donate_atoms} supply atoms back (simulated off-chain recovery) ==");
    println!("    (SECOND token transfer: curator supply tokens -> market vault;");
    println!("     suppliers recover pro-rata via the share price.)");
    let tx = client
        .with_signer(curator)
        .tx_builder()
        .donate_supply(&market, donate_atoms)
        .await?;
    let tx6 = send_tx(&rpc, network, &curator, tx, "donate_supply").await?;
    client.full_reload().await?;
    let s6 = snap!().await;
    print_snapshot("after step 6", &s5, &s6);
    let recovery = s6.redeemable.saturating_sub(s5.redeemable);
    // Share-price accounting rounds down: crediting `donate_atoms` over a large
    // share pool loses sub-atom dust per share, so allow a small proportional
    // tolerance (1 ppm + a floor) rather than exact equality.
    let recovery_tol = donate_atoms / 1_000_000 + 100;
    let pass6 = recovery + recovery_tol >= donate_atoms;
    println!(
        "    ASSERT supplier A recovers by ~donation -> after_socialize={} after_donate={} recovery={recovery} (donated={donate_atoms}) => {}",
        s5.redeemable,
        s6.redeemable,
        pv(pass6)
    );
    let net_loss = s_before.redeemable.saturating_sub(s6.redeemable);
    println!("    NET supplier A loss (debt - add-back) = {net_loss} atoms");
    results.push((
        format!("6 donate_supply (tx {tx6})"),
        pass6,
        format!("recovery={recovery} net_loss={net_loss}"),
    ));

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
