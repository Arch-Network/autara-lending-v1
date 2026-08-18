//! Autara (lending) deploy tool.
//!
//! Everything is env-driven (see [`DeployConfig::from_env`]). Run with
//! `--dry-run` (or `DRY_RUN=1`) to print every derived address and preflight
//! check without touching the chain. A real run deploys the `autara-program`
//! and `autara-oracle` ELFs, creates the global config, and writes a
//! `DeploymentArtifact` (addresses + tx ids only).
//!
//! Phase 1 is TESTNET-FIRST. The structure mirrors the CLAMM `clamm-deploy`
//! crate so the two stay consistent.

mod artifact;
mod config;
// Lives in the lib target (shared with autara-client's upgrade bins); re-exported
// so the bin's own modules keep resolving it as `crate::elf_upload`.
pub use autara_deploy::elf_upload;
mod rpc;
mod steps;
mod verify;

use std::path::Path;
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use arch_program::pubkey::Pubkey;
use arch_sdk::{AsyncArchRpcClient, Config as ArchConfig};
use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};

use artifact::{DeploymentArtifact, TokenRecord};
use config::{pyth_feed_for_label, DeployConfig, Network};
use rpc::{load_keypair, RpcContext};

#[derive(Parser, Debug)]
#[command(
    name = "autara-deploy",
    about = "Deploy Autara lending to Arch Network"
)]
struct Cli {
    /// Print all derived addresses and preflight checks, then exit without
    /// sending any transaction.
    #[arg(long)]
    dry_run: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Read-only: print the native lamport balance of an owner (defaults to the
    /// admin keypair, then the deployer keypair). Sends NO transaction.
    CheckBalance(CheckBalanceArgs),
    /// Faucet-fund the deployer + admin keypairs for the configured network.
    /// Sends ONLY faucet airdrops — no deploy/init/program transactions. Uses
    /// the same env-driven key paths as the deploy flow.
    Fund(FundArgs),
    /// Read-only end-to-end verification of a deployment: asserts the program,
    /// global config, mints, markets, and oracle freshness on-chain (and,
    /// optionally, the running server JSON-RPC). Sends NO transaction.
    Verify(VerifyArgs),
}

#[derive(Args, Debug)]
struct VerifyArgs {
    /// Optional autara-server JSON-RPC url to probe (get_all_market_ids /
    /// get_market_by_id). Omit to skip the server check.
    #[arg(long)]
    server_url: Option<String>,

    /// Assert each mint's supply >= its configured mint_amount (use only when
    /// the initial-supply minting step was expected to have run).
    #[arg(long, default_value_t = false)]
    expect_supply: bool,
}

#[derive(Args, Debug)]
struct FundArgs {
    /// Number of faucet airdrops to request for the deployer (program ELFs are
    /// large, so several airdrops are typically needed to cover the upload).
    #[arg(long, default_value_t = 5)]
    deployer_rounds: u32,

    /// Number of faucet airdrops to request for the admin (payer/signer for
    /// create_global_config).
    #[arg(long, default_value_t = 1)]
    admin_rounds: u32,
}

#[derive(Args, Debug)]
struct CheckBalanceArgs {
    /// Owner pubkey (hex). Defaults to the ADMIN_KEY_PATH (then DEPLOYER_KEY_PATH) keypair.
    #[arg(long)]
    owner: Option<String>,

    /// Arch RPC url. Defaults to ARCH_RPC_URL or the network default.
    #[arg(long)]
    rpc_url: Option<String>,

    /// Network (localnet|testnet). Defaults to NETWORK or localnet.
    #[arg(long)]
    network: Option<String>,
}

/// Read a boolean env var, defaulting to `default` when unset/empty.
fn env_flag(key: &str, default: bool) -> bool {
    match std::env::var(key).ok().filter(|v| !v.trim().is_empty()) {
        None => default,
        Some(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        ),
    }
}

/// SHA-256 of a file's bytes, hex-encoded.
fn sha256_file(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading ELF at {path}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// Best-effort short git commit of the working tree.
fn git_short_commit() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Resolve the network from an explicit flag, the `NETWORK` env var, or the
/// localnet default.
fn resolve_network(explicit: Option<String>) -> Result<Network> {
    explicit
        .or_else(|| {
            std::env::var("NETWORK")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "localnet".to_string())
        .parse()
}

/// Print the mainnet manual-funding banner. Mainnet has no faucet, so the
/// operator must fund the deployer + admin out-of-band before a real deploy.
/// Shows both addresses and their current on-chain balances (or "account not
/// found") so the operator knows exactly what to fund. Printed in both dry-run
/// and real runs; the actual halt-on-insufficient-funds gate lives in the real
/// run path.
fn print_mainnet_funding_banner(
    deployer: Pubkey,
    deployer_balance: Option<u64>,
    admin: Pubkey,
    admin_balance: Option<u64>,
    min_deployer_lamports: u64,
    mint_supply_note: bool,
) {
    let fmt = |b: Option<u64>| match b {
        Some(v) => v.to_string(),
        None => "account not found".to_string(),
    };
    println!("================ MANUAL FUNDING REQUIRED (MAINNET) ================");
    println!("Fund these addresses with native lamports before deploying:");
    println!(
        "  deployer: {deployer}   balance: {}",
        fmt(deployer_balance)
    );
    println!("  admin:    {admin}   balance: {}", fmt(admin_balance));
    println!("  (deployer needs >= {min_deployer_lamports} lamports; admin must be funded > 0)");
    if mint_supply_note {
        println!(
            "  note: STEP_MINT_INITIAL_SUPPLY is set — the mint authority account(s) also \
             need manual funding"
        );
    }
    println!("===================================================================");
}

/// `check-balance` subcommand: read-only lamport-balance report.
async fn run_check_balance(args: CheckBalanceArgs) -> Result<()> {
    let network = resolve_network(args.network)?;
    let rpc_url = args
        .rpc_url
        .or_else(|| {
            std::env::var("ARCH_RPC_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .map(Ok)
        .unwrap_or_else(|| network.default_rpc_url())?;

    let owner = match args.owner {
        Some(o) => Pubkey::from_str(o.trim())
            .map_err(|e| anyhow!("invalid --owner pubkey '{o}': {e:?}"))?,
        None => {
            let path = std::env::var("ADMIN_KEY_PATH")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("DEPLOYER_KEY_PATH")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .context("--owner not given and ADMIN_KEY_PATH/DEPLOYER_KEY_PATH are unset")?;
            load_keypair(&path)?.1
        }
    };

    let config = ArchConfig {
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network: network.bitcoin_network()?,
        arch_node_url: rpc_url.clone(),
        titan_url: String::new(),
    };
    let rpc = AsyncArchRpcClient::new(&config);

    println!("network:  {}", network.as_str());
    println!("rpc_url:  {rpc_url}");
    println!("owner:    {owner}");
    match rpc.read_account_info(owner).await {
        Ok(info) => println!("balance:  {} lamports", info.lamports),
        Err(_) => println!("balance:  (account not found)"),
    }
    Ok(())
}

/// `fund` subcommand: faucet-fund the deployer + admin keypairs. Sends ONLY
/// faucet airdrops (no deploy/init transactions), so it is safe to run while
/// preparing a fresh deploy. Funding brand-new addresses is non-destructive.
async fn run_fund(args: FundArgs) -> Result<()> {
    let cfg = DeployConfig::from_env()?;
    if !cfg.network.has_faucet() {
        bail!(
            "network '{}' has no faucet; fund the accounts manually",
            cfg.network.as_str()
        );
    }

    let (deployer_kp, deployer_pubkey) = load_keypair(&cfg.deployer_key_path)?;
    let (admin_kp, admin_pubkey) = load_keypair(&cfg.admin_key_path)?;

    // The admin keypair doubles as the RpcContext payer; `fund_with_faucet`
    // only uses the shared RPC client, so the payer role is irrelevant here.
    let ctx = RpcContext::new(cfg.arch_config()?, admin_kp, admin_pubkey);

    println!("network:  {}", cfg.network.as_str());
    println!("rpc_url:  {}", cfg.arch_rpc_url);
    println!("deployer: {deployer_pubkey}");
    println!("admin:    {admin_pubkey}");

    for i in 0..args.deployer_rounds {
        ctx.fund_with_faucet(&deployer_kp).await?;
        println!(
            "faucet -> deployer (round {}/{})",
            i + 1,
            args.deployer_rounds
        );
    }
    for i in 0..args.admin_rounds {
        ctx.fund_with_faucet(&admin_kp).await?;
        println!("faucet -> admin    (round {}/{})", i + 1, args.admin_rounds);
    }

    match ctx.balance(deployer_pubkey).await {
        Some(b) => println!("deployer_balance: {b} lamports"),
        None => println!("deployer_balance: (account not found)"),
    }
    match ctx.balance(admin_pubkey).await {
        Some(b) => println!("admin_balance:    {b} lamports"),
        None => println!("admin_balance:    (account not found)"),
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // A single-thread runtime drives the async reads/sends. Synchronous ELF
    // upload runs OUTSIDE `block_on` (blocking ArchRpcClient).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    if let Some(command) = cli.command {
        return match command {
            Command::CheckBalance(args) => rt.block_on(run_check_balance(args)),
            Command::Fund(args) => rt.block_on(run_fund(args)),
            Command::Verify(args) => {
                let cfg = DeployConfig::from_env()?;
                rt.block_on(verify::run(&cfg, args.server_url, args.expect_supply))
            }
        };
    }

    let dry_run = cli.dry_run || env_flag("DRY_RUN", false);
    let cfg = DeployConfig::from_env()?;

    // Load keypairs. Keypair is Copy, so the same key can serve multiple roles.
    let (program_kp, program_pubkey) = load_keypair(&cfg.program_key_path)?;
    let (oracle_kp, oracle_pubkey) = load_keypair(&cfg.oracle_key_path)?;
    let (deployer_kp, deployer_pubkey) = load_keypair(&cfg.deployer_key_path)?;
    let (admin_kp, admin_default_pubkey) = load_keypair(&cfg.admin_key_path)?;

    let admin = cfg.admin.unwrap_or(admin_default_pubkey);
    let fee_receiver = cfg.fee_receiver.unwrap_or(admin);

    // The global-config payer/signer is the admin keypair.
    let ctx = RpcContext::new(cfg.arch_config()?, admin_kp, admin_default_pubkey);

    // Step gating (default-on, mirroring CLAMM's create-pool/seed convention).
    // The CI engine sets these explicitly per action; the defaults only matter
    // for manual/local runs.
    let step_deploy_program = env_flag("STEP_DEPLOY_PROGRAM", true);
    let step_deploy_oracle = env_flag("STEP_DEPLOY_ORACLE", true);
    let step_init_config = env_flag("STEP_INIT_CONFIG", true);
    let step_token_setup = env_flag("STEP_TOKEN_SETUP", true);
    // Minting an initial supply is sensitive (requires a mint authority) and is
    // therefore OPT-IN: off by default and a no-op unless a mint authority key
    // is also resolvable. The step itself verifies the on-chain mint_authority.
    let step_mint_initial_supply = env_flag("STEP_MINT_INITIAL_SUPPLY", false);
    let step_create_market = env_flag("STEP_CREATE_MARKET", true);

    // Markets are curated by the admin keypair (the signer we hold), so the
    // curator == the global-config payer/signer (`ctx.payer_pubkey()`).
    let curator = admin_default_pubkey;
    let market_pairs = cfg.effective_market_pairs();

    // ----- Preflight -----
    println!("== Autara deploy preflight ({}) ==", cfg.network.as_str());
    println!("arch_rpc_url:      {}", cfg.arch_rpc_url);
    println!("program_id:        {program_pubkey}");
    println!("oracle_id:         {oracle_pubkey}");
    println!("deployer:          {deployer_pubkey}");
    println!("admin:             {admin}");
    println!("fee_receiver:      {fee_receiver}");
    println!("protocol_fee_bps:  {}", cfg.protocol_fee_share_bps);

    // The on-chain `autara-program` derives the global-config PDA and runs its
    // ownership checks against a COMPILED-IN id (`autara_program::id()`), not
    // the runtime program id. The client, however, derives PDAs from the
    // deployed program key. If the two disagree, create_global_config / market
    // instructions target the wrong PDA and fail. Guard it here — fatal on a
    // real run that touches the lending program; warn-only for oracle-only
    // uploads (the oracle is position-independent and needs no such guard).
    let compiled_id = autara_program::id();
    let touches_lending_program =
        step_deploy_program || step_init_config || step_token_setup || step_create_market;
    if program_pubkey != compiled_id {
        eprintln!("WARNING: program keypair pubkey != compiled autara_program::id()");
        eprintln!("  program keypair      : {program_pubkey}");
        eprintln!("  autara_program::id() : {compiled_id}");
        eprintln!("  Fix: deploy with the keypair whose pubkey == autara_program::id(),");
        eprintln!("       or update id() in programs/autara-program/src/lib.rs and rebuild.");
        if !dry_run && touches_lending_program {
            bail!(
                "program id mismatch: create_global_config/market would target the wrong PDA \
                 (compiled {compiled_id}, deploying {program_pubkey})"
            );
        }
        if !touches_lending_program {
            println!("program_id_guard:  skipped (oracle-only / no lending-program steps enabled)");
        }
    } else {
        println!("program_id_guard:  ok (matches autara_program::id())");
    }

    // Mainnet-only footgun guards: no faucet on mainnet, and the configured token
    // mints must not still be the testnet PLACEHOLDER mints shipped in
    // autara.mainnet.env. Fatal on a REAL run; warn-only in dry-run so the
    // preview still completes (a real run would be refused).
    let mainnet_violations = cfg.mainnet_safety_violations();
    if mainnet_violations.is_empty() {
        if cfg.network == Network::Mainnet {
            println!("mainnet_guard:     ok (no faucet, no placeholder mints)");
        }
    } else {
        eprintln!(
            "MAINNET SAFETY: {} check(s) failed:",
            mainnet_violations.len()
        );
        for v in &mainnet_violations {
            eprintln!("  - {v}");
        }
        if !dry_run {
            bail!(
                "refusing a REAL mainnet run: {} safety check(s) failed (see above)",
                mainnet_violations.len()
            );
        }
        eprintln!("  (dry-run: warnings only — a REAL run would be REFUSED by these guards)");
    }

    let (global_config_pda, _) = autara_lib::pda::find_global_config_pda(&program_pubkey);
    println!("global_config_pda: {global_config_pda}");

    for t in &cfg.tokens {
        println!(
            "token {:<6}     mint={} decimals={}",
            t.label, t.mint, t.decimals
        );
    }

    // ----- Market / token-setup preview (derived addresses only) -----
    println!("step_token_setup:  {step_token_setup}");
    println!("step_mint_supply:  {step_mint_initial_supply}");
    if step_mint_initial_supply {
        for t in &cfg.tokens {
            match cfg.mint_authority_for(t) {
                Some(path) => println!(
                    "mint {:<6}     amount={} authority_key={path}",
                    t.label, t.mint_amount
                ),
                None => println!(
                    "mint {:<6}     (SKIP — no MINT_AUTHORITY_KEY_PATH[_{}])",
                    t.label,
                    t.label.to_uppercase()
                ),
            }
        }
    }
    println!("step_create_market:{step_create_market}");
    if step_create_market {
        println!("curator:           {curator}");
        println!("oracle_program_id: {oracle_pubkey}");
        println!("lending_fee_bps:   {}", cfg.lending_market_fee_bps);
        println!(
            "market_params:     max_ltv={} unhealthy_ltv={} liquidation_bonus={} max_utilisation={}",
            cfg.market_params.max_ltv,
            cfg.market_params.unhealthy_ltv,
            cfg.market_params.liquidation_bonus,
            cfg.market_params.max_utilisation_rate
        );
        if market_pairs.is_empty() {
            println!(
                "markets:           (none — set MARKET_PAIRS or configure TOKENS with Pyth feeds)"
            );
        }
        for pair in &market_pairs {
            match (
                cfg.token_by_label(&pair.supply_label),
                cfg.token_by_label(&pair.collateral_label),
            ) {
                (Some(supply), Some(collateral)) => {
                    let feeds_ok = pyth_feed_for_label(&supply.label).is_some()
                        && pyth_feed_for_label(&collateral.label).is_some();
                    let market =
                        steps::derive_market_pda(program_pubkey, curator, supply, collateral, 0);
                    println!(
                        "market {:>4}/{:<4}  {market}{}",
                        pair.supply_label,
                        pair.collateral_label,
                        if feeds_ok {
                            ""
                        } else {
                            "  (NO Pyth feed — will be skipped)"
                        }
                    );
                }
                _ => println!(
                    "market {:>4}/{:<4}  (UNRESOLVED — label not in TOKENS)",
                    pair.supply_label, pair.collateral_label
                ),
            }
        }
    }

    for (label, path) in [
        ("program_elf", &cfg.program_elf_path),
        ("oracle_elf", &cfg.oracle_elf_path),
    ] {
        if Path::new(path).exists() {
            println!("{label:<14}   {path} (present)");
        } else {
            println!("{label:<14}   {path} (MISSING — run the SBF build)");
        }
    }

    rt.block_on(async {
        match ctx.rpc_reachable().await {
            Ok(()) => println!("rpc_reachable:     yes"),
            Err(e) => println!("rpc_reachable:     NO ({e})"),
        }
        let deployer_balance = ctx.balance(deployer_pubkey).await;
        match deployer_balance {
            Some(b) => println!("deployer_balance:  {b} lamports"),
            None => println!("deployer_balance:  (account not found)"),
        }
        let admin_balance = ctx.balance(admin).await;
        match admin_balance {
            Some(b) => println!("admin_balance:     {b} lamports"),
            None => println!("admin_balance:     (account not found)"),
        }
        // Mainnet has no faucet: surface the addresses so the operator can fund
        // them manually. Printed in dry-run and real runs alike (the real run
        // additionally halts if funding is insufficient — see below).
        if cfg.network == Network::Mainnet {
            print_mainnet_funding_banner(
                deployer_pubkey,
                deployer_balance,
                admin,
                admin_balance,
                cfg.min_deployer_lamports,
                step_mint_initial_supply,
            );
        }
        if step_deploy_program {
            let live = ctx.is_executable(program_pubkey).await;
            println!(
                "program_on_chain:  {}",
                if live { "executable" } else { "not deployed" }
            );
        }
        if step_deploy_oracle {
            let live = ctx.is_executable(oracle_pubkey).await;
            println!(
                "oracle_on_chain:   {}",
                if live { "executable" } else { "not deployed" }
            );
        }
    });

    if dry_run {
        println!("\n[dry-run] No transactions sent. Nothing written on-chain.");
        return Ok(());
    }

    // ----- Real run -----
    let mut artifact = DeploymentArtifact {
        network: cfg.network.as_str().to_string(),
        arch_rpc_url: cfg.arch_rpc_url.clone(),
        deployed_at_unix: DeploymentArtifact::now_unix(),
        program_id: program_pubkey.to_string(),
        oracle_id: oracle_pubkey.to_string(),
        build_commit: git_short_commit(),
        program_elf_path: cfg.program_elf_path.clone(),
        program_elf_sha256: None,
        oracle_elf_path: cfg.oracle_elf_path.clone(),
        oracle_elf_sha256: None,
        deployer: deployer_pubkey.to_string(),
        admin: admin.to_string(),
        fee_receiver: fee_receiver.to_string(),
        protocol_fee_share_bps: cfg.protocol_fee_share_bps,
        global_config: None,
        tokens: cfg
            .tokens
            .iter()
            .map(|t| TokenRecord {
                label: t.label.clone(),
                mint: t.mint.to_string(),
                decimals: t.decimals,
                mint_amount: t.mint_amount,
                faucet_amount: t.faucet_amount,
            })
            .collect(),
        markets: Vec::new(),
        transactions: Vec::new(),
    };

    // Fund the deployer + admin. Localnet/testnet use the faucet (large ELFs =>
    // several airdrops). Mainnet has no faucet: gate on the operator having
    // funded both accounts out-of-band and halt otherwise. (This code is only
    // reached on a REAL run — dry-run returns above — so the gate never fires
    // in dry-run; the funding banner was already printed in preflight.)
    if cfg.use_faucet {
        rt.block_on(async {
            for _ in 0..5 {
                ctx.fund_with_faucet(&deployer_kp).await?;
            }
            ctx.fund_with_faucet(&admin_kp).await?;
            Ok::<_, anyhow::Error>(())
        })?;
        println!("faucet: funded deployer + admin");
    } else if cfg.network == Network::Mainnet {
        rt.block_on(async {
            let deployer_balance = ctx.balance(deployer_pubkey).await;
            let admin_balance = ctx.balance(admin).await;
            // Deployer must cover the large ELF uploads; admin only signs the
            // cheap create_global_config / market instructions, so requiring it
            // to merely exist and be non-zero is sufficient.
            let deployer_ok = deployer_balance.is_some_and(|b| b >= cfg.min_deployer_lamports);
            let admin_ok = admin_balance.is_some_and(|b| b > 0);
            if !deployer_ok || !admin_ok {
                bail!("Insufficient funds — fund the address(es) above and re-run this deploy.");
            }
            Ok::<_, anyhow::Error>(())
        })?;
        println!("mainnet: deployer + admin funding verified");
    }

    // Deploy the programs (synchronous ELF upload — outside the runtime).
    if step_deploy_program {
        artifact.program_elf_sha256 = Some(sha256_file(&cfg.program_elf_path)?);
        ctx.deploy_program(
            "autara_program".to_string(),
            program_kp,
            deployer_kp,
            cfg.program_elf_path.clone(),
        )?;
        println!("deployed autara-program {program_pubkey}");
    } else if Path::new(&cfg.program_elf_path).exists() {
        artifact.program_elf_sha256 = sha256_file(&cfg.program_elf_path).ok();
    }

    if step_deploy_oracle {
        artifact.oracle_elf_sha256 = Some(sha256_file(&cfg.oracle_elf_path)?);
        ctx.deploy_program(
            "autara_oracle".to_string(),
            oracle_kp,
            deployer_kp,
            cfg.oracle_elf_path.clone(),
        )?;
        println!("deployed autara-oracle {oracle_pubkey}");
    } else if Path::new(&cfg.oracle_elf_path).exists() {
        artifact.oracle_elf_sha256 = sha256_file(&cfg.oracle_elf_path).ok();
    }

    // Create the global config.
    if step_init_config {
        let pda = rt.block_on(steps::create_global_config(
            &ctx,
            program_pubkey,
            admin,
            fee_receiver,
            cfg.protocol_fee_share_bps,
            &mut artifact,
        ))?;
        println!("global config {pda}");
    }

    // Ensure the configured token mints exist before creating markets.
    if step_token_setup {
        rt.block_on(steps::ensure_token_mints(&ctx, &cfg.tokens))?;
        println!("token setup: {} mint(s) ensured", cfg.tokens.len());
    }

    // Mint the configured initial supply to the authority's ATA (opt-in). Each
    // token needs a resolvable mint authority; the step verifies the on-chain
    // mint_authority and refuses on mismatch. Tokens without an authority key
    // are skipped (logged), not fatal.
    if step_mint_initial_supply {
        for token in &cfg.tokens {
            if token.mint_amount == 0 {
                continue;
            }
            let Some(authority_path) = cfg.mint_authority_for(token) else {
                println!(
                    "mint {}: no authority key configured — skipping",
                    token.label
                );
                continue;
            };
            let (authority_kp, authority_pubkey) = load_keypair(authority_path)?;
            let authority_ctx = RpcContext::new(cfg.arch_config()?, authority_kp, authority_pubkey);
            if cfg.use_faucet {
                rt.block_on(async {
                    for _ in 0..3 {
                        if let Err(e) = authority_ctx.fund_with_faucet(&authority_kp).await {
                            eprintln!("faucet -> mint authority did not confirm cleanly: {e}");
                        }
                    }
                });
            }
            let recipient = cfg.mint_recipient.unwrap_or(authority_pubkey);
            rt.block_on(steps::mint_initial_supply(
                &authority_ctx,
                token,
                recipient,
                &mut artifact,
            ))?;
        }
    }

    // Create a lending market for each configured pair (idempotent).
    if step_create_market {
        for pair in &market_pairs {
            let supply = cfg.token_by_label(&pair.supply_label).with_context(|| {
                format!(
                    "market pair supply label '{}' not in TOKENS",
                    pair.supply_label
                )
            })?;
            let collateral = cfg
                .token_by_label(&pair.collateral_label)
                .with_context(|| {
                    format!(
                        "market pair collateral label '{}' not in TOKENS",
                        pair.collateral_label
                    )
                })?;
            if pyth_feed_for_label(&supply.label).is_none()
                || pyth_feed_for_label(&collateral.label).is_none()
            {
                println!(
                    "skipping market {}/{}: no Pyth feed mapping",
                    pair.supply_label, pair.collateral_label
                );
                continue;
            }
            let market = rt.block_on(steps::create_market(
                &ctx,
                program_pubkey,
                oracle_pubkey,
                curator,
                pair,
                supply,
                collateral,
                cfg.lending_market_fee_bps,
                cfg.market_params,
                0,
                &mut artifact,
            ))?;
            println!(
                "market {}/{} {market}",
                pair.supply_label, pair.collateral_label
            );
        }
    }

    artifact.write(&cfg.output_path)?;
    println!("\n== Deploy complete ==");
    println!("artifact: {}", cfg.output_path);
    println!("transactions recorded: {}", artifact.transactions.len());

    Ok(())
}
