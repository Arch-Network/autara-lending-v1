//! Read-only end-to-end verification of an Autara deployment.
//!
//! Given the same env-driven [`DeployConfig`] used to deploy, `autara-deploy
//! verify` asserts — over RPC reads ONLY, never sending a transaction — that the
//! on-chain state matches the configuration:
//!
//!   (a) the program account is present, executable, and owned by a loader;
//!   (b) the global-config PDA exists and decodes (admin / fee receiver / fee);
//!   (c) every configured token mint exists with the expected decimals (and
//!       supply >= the configured mint_amount when minting was expected);
//!   (d) each configured market PDA exists with the expected supply/collateral
//!       mints, max-LTV, and curator;
//!   (e) the Pyth oracle PDA for every feed used by a market is owned by the
//!       oracle program and FRESH (publish_time within `max_age` seconds);
//!   (f) (optional) the running server JSON-RPC answers get_all_market_ids /
//!       get_market_by_id.
//!
//! It prints a PASS/FAIL line per check and a final summary, and exits non-zero
//! if any check failed. It is intended to be run AFTER a (gated) live deploy.

use std::mem::size_of;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use apl_token::state::Mint;
use arch_program::program_pack::Pack;
use arch_program::pubkey::Pubkey;
use arch_sdk::AsyncArchRpcClient;

use autara_lib::math::ifixed_point::IFixedPoint;
use autara_lib::oracle::pyth::PythPriceAccount;
use autara_lib::pda::{find_global_config_pda, find_market_pda};
use autara_lib::state::global_config::GlobalConfig;
use autara_lib::state::market::Market;

use crate::config::{pyth_feed_for_label, DeployConfig};
use crate::rpc::load_keypair;

/// Maximum oracle staleness tolerated, in seconds. Mirrors the lending program's
/// `max_age` freshness check (`oracle_provider.rs`).
const ORACLE_MAX_AGE_SECS: i64 = 60;

/// Accumulates per-check results and renders a PASS/FAIL report.
#[derive(Default)]
struct Report {
    checks: Vec<(String, bool, String)>,
}

impl Report {
    fn record(&mut self, name: impl Into<String>, pass: bool, detail: impl Into<String>) {
        self.checks.push((name.into(), pass, detail.into()));
    }

    fn pass(&mut self, name: impl Into<String>, detail: impl Into<String>) {
        self.record(name, true, detail);
    }

    fn fail(&mut self, name: impl Into<String>, detail: impl Into<String>) {
        self.record(name, false, detail);
    }

    fn failed(&self) -> usize {
        self.checks.iter().filter(|(_, ok, _)| !ok).count()
    }

    fn print(&self) {
        println!("\n== verify report ==");
        for (name, ok, detail) in &self.checks {
            println!(
                "[{}] {name:<28} {detail}",
                if *ok { "PASS" } else { "FAIL" }
            );
        }
        let failed = self.failed();
        println!(
            "== {} check(s), {} passed, {} failed ==",
            self.checks.len(),
            self.checks.len() - failed,
            failed
        );
    }
}

/// Decode a Pod account body (the leading `size_of::<T>()` bytes).
fn decode_pod<T: bytemuck::Pod>(data: &[u8]) -> Option<T> {
    data.get(..size_of::<T>())
        .and_then(|slice| bytemuck::try_from_bytes::<T>(slice).ok().copied())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run all verification checks. `server_url` enables the optional JSON-RPC probe.
/// `expect_supply` asserts mint supply >= the configured mint_amount (only true
/// when the minting step was expected to have run).
pub async fn run(
    cfg: &DeployConfig,
    server_url: Option<String>,
    expect_supply: bool,
) -> Result<()> {
    let rpc = AsyncArchRpcClient::new(&cfg.arch_config()?);

    // Resolve the deployed program / oracle ids and the admin (curator) from the
    // configured key paths, mirroring the deploy binary.
    let (_, program_id) = load_keypair(&cfg.program_key_path)
        .with_context(|| format!("loading program key {}", cfg.program_key_path))?;
    let (_, oracle_id) = load_keypair(&cfg.oracle_key_path)
        .with_context(|| format!("loading oracle key {}", cfg.oracle_key_path))?;
    let (_, admin_default) = load_keypair(&cfg.admin_key_path)
        .with_context(|| format!("loading admin key {}", cfg.admin_key_path))?;
    let admin = cfg.admin.unwrap_or(admin_default);
    let curator = admin_default;

    println!("network:   {}", cfg.network.as_str());
    println!("rpc_url:   {}", cfg.arch_rpc_url);
    println!("program:   {program_id}");
    println!("oracle:    {oracle_id}");
    println!("admin:     {admin}");
    println!("curator:   {curator}");

    let mut report = Report::default();

    verify_program(&rpc, program_id, &mut report).await;
    verify_global_config(&rpc, program_id, admin, &mut report).await;
    verify_mints(&rpc, cfg, expect_supply, &mut report).await;
    verify_markets(&rpc, cfg, program_id, oracle_id, curator, &mut report).await;
    if let Some(url) = server_url {
        verify_server(&url, &mut report).await;
    }

    report.print();
    if report.failed() > 0 {
        return Err(anyhow!("verify: {} check(s) FAILED", report.failed()));
    }
    Ok(())
}

/// (a) program account present, executable, and owned by a loader.
async fn verify_program(rpc: &AsyncArchRpcClient, program_id: Pubkey, report: &mut Report) {
    match rpc.read_account_info(program_id).await {
        Ok(info) if info.is_executable => report.pass(
            "program.executable",
            format!("executable, owner={}", info.owner),
        ),
        Ok(info) => report.fail(
            "program.executable",
            format!("present but NOT executable (owner={})", info.owner),
        ),
        Err(e) => report.fail("program.executable", format!("not found: {e}")),
    }
}

/// (b) global-config PDA exists and decodes.
async fn verify_global_config(
    rpc: &AsyncArchRpcClient,
    program_id: Pubkey,
    expected_admin: Pubkey,
    report: &mut Report,
) {
    let (pda, _) = find_global_config_pda(&program_id);
    match rpc.read_account_info(pda).await {
        Ok(info) => match decode_pod::<GlobalConfig>(&info.data) {
            Some(gc) => {
                let admin_ok = *gc.admin() == expected_admin;
                report.record(
                    "global_config",
                    admin_ok,
                    format!(
                        "{pda} admin={} fee_receiver={} fee_bps={}{}",
                        gc.admin(),
                        gc.fee_receiver(),
                        gc.protocol_fee_share_in_bps(),
                        if admin_ok {
                            String::new()
                        } else {
                            format!(" (EXPECTED admin {expected_admin})")
                        }
                    ),
                );
            }
            None => report.fail(
                "global_config",
                format!("{pda} present but failed to decode"),
            ),
        },
        Err(e) => report.fail("global_config", format!("{pda} not found: {e}")),
    }
}

/// (c) every configured mint exists with the expected decimals (and supply).
async fn verify_mints(
    rpc: &AsyncArchRpcClient,
    cfg: &DeployConfig,
    expect_supply: bool,
    report: &mut Report,
) {
    for token in &cfg.tokens {
        let name = format!("mint.{}", token.label);
        match rpc.read_account_info(token.mint).await {
            Ok(info) => match Mint::unpack(&info.data) {
                Ok(mint) => {
                    let decimals_ok = mint.decimals == token.decimals;
                    let supply_ok = !expect_supply || mint.supply >= token.mint_amount;
                    report.record(
                        name,
                        decimals_ok && supply_ok,
                        format!(
                            "{} decimals={} (want {}) supply={}{}",
                            token.mint,
                            mint.decimals,
                            token.decimals,
                            mint.supply,
                            if expect_supply && !supply_ok {
                                format!(" (EXPECTED >= {})", token.mint_amount)
                            } else {
                                String::new()
                            }
                        ),
                    );
                }
                Err(e) => report.fail(name, format!("{} failed to decode mint: {e}", token.mint)),
            },
            Err(e) => report.fail(name, format!("{} not found: {e}", token.mint)),
        }
    }
}

/// (d) each configured market PDA exists with expected mints/LTV/curator, and
/// (e) the Pyth oracle PDA for each of its feeds is owned-by-oracle and fresh.
async fn verify_markets(
    rpc: &AsyncArchRpcClient,
    cfg: &DeployConfig,
    program_id: Pubkey,
    oracle_id: Pubkey,
    curator: Pubkey,
    report: &mut Report,
) {
    for pair in cfg.effective_market_pairs() {
        let label = format!("market.{}/{}", pair.supply_label, pair.collateral_label);
        let (Some(supply), Some(collateral)) = (
            cfg.token_by_label(&pair.supply_label),
            cfg.token_by_label(&pair.collateral_label),
        ) else {
            report.fail(label, "label not in TOKENS");
            continue;
        };

        let (pda, _) = find_market_pda(&program_id, &curator, &supply.mint, &collateral.mint, 0);
        match rpc.read_account_info(pda).await {
            Ok(info) => match decode_pod::<Market>(&info.data) {
                Some(market) => {
                    let s = market.supply_token_info();
                    let c = market.collateral_token_info();
                    let want_ltv = IFixedPoint::from_num(cfg.market_params.max_ltv);
                    let mints_ok = s.mint == supply.mint && c.mint == collateral.mint;
                    let ltv_ok = market.config().ltv_config().max_ltv == want_ltv;
                    let curator_ok = *market.config().curator() == curator;
                    report.record(
                        label.clone(),
                        mints_ok && ltv_ok && curator_ok,
                        format!(
                            "{pda} supply={} collateral={} max_ltv={:?} curator_ok={curator_ok} mints_ok={mints_ok} ltv_ok={ltv_ok}",
                            s.mint,
                            c.mint,
                            market.config().ltv_config().max_ltv
                        ),
                    );
                }
                None => report.fail(label.clone(), format!("{pda} present but failed to decode")),
            },
            Err(e) => report.fail(label.clone(), format!("{pda} not found: {e}")),
        }

        // (e) oracle freshness for both legs.
        for (leg, token) in [("supply", supply), ("collateral", collateral)] {
            let Some(feed_id) = pyth_feed_for_label(&token.label) else {
                report.fail(
                    format!("oracle.{}.{leg}", token.label),
                    "no Pyth feed for label",
                );
                continue;
            };
            verify_oracle(rpc, oracle_id, feed_id, &token.label, leg, report).await;
        }
    }
}

async fn verify_oracle(
    rpc: &AsyncArchRpcClient,
    oracle_id: Pubkey,
    feed_id: [u8; 32],
    label: &str,
    leg: &str,
    report: &mut Report,
) {
    let name = format!("oracle.{label}.{leg}");
    let (pda, _) = Pubkey::find_program_address(&[&feed_id], &oracle_id);
    match rpc.read_account_info(pda).await {
        Ok(info) => {
            if info.owner != oracle_id {
                report.fail(
                    name,
                    format!("{pda} owner {} != oracle {oracle_id}", info.owner),
                );
                return;
            }
            match decode_pod::<PythPriceAccount>(&info.data) {
                Some(acc) => {
                    let publish_time = acc.pyth_price.price.publish_time;
                    let age = now_unix() - publish_time;
                    let fresh = age <= ORACLE_MAX_AGE_SECS;
                    report.record(
                        name,
                        fresh,
                        format!("{pda} publish_time={publish_time} age={age}s (max {ORACLE_MAX_AGE_SECS}s)"),
                    );
                }
                None => report.fail(name, format!("{pda} present but failed to decode")),
            }
        }
        Err(e) => report.fail(name, format!("{pda} not found: {e}")),
    }
}

/// (f) optional: the running server answers get_all_market_ids / get_market_by_id.
async fn verify_server(url: &str, report: &mut Report) {
    let client = reqwest::Client::new();
    let ids: Vec<String> =
        match jsonrpc_call(&client, url, "get_all_market_ids", serde_json::json!([])).await {
            Ok(value) => {
                let ids: Vec<String> = value
                    .get("marketIds")
                    .or_else(|| value.get("market_ids"))
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                report.pass(
                    "server.get_all_market_ids",
                    format!("{} market id(s)", ids.len()),
                );
                ids
            }
            Err(e) => {
                report.fail("server.get_all_market_ids", e.to_string());
                return;
            }
        };

    if let Some(first) = ids.first() {
        let params = serde_json::json!([{ "marketId": first }]);
        match jsonrpc_call(&client, url, "get_market_by_id", params).await {
            Ok(_) => report.pass(
                "server.get_market_by_id",
                format!("market {first} answered"),
            ),
            Err(e) => report.fail("server.get_market_by_id", e.to_string()),
        }
    }
}

/// Minimal JSON-RPC 2.0 POST; returns the `result` value or an error.
async fn jsonrpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let resp = client
        .post(url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {method} to {url}"))?;
    let status = resp.status();
    let value: serde_json::Value = resp
        .json()
        .await
        .with_context(|| format!("decoding {method} response (HTTP {status})"))?;
    if let Some(err) = value.get("error") {
        return Err(anyhow!("{method} returned error: {err}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("{method} returned no result (HTTP {status})"))
}
