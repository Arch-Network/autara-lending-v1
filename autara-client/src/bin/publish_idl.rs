//! Publish Autara's IDL on-chain by driving our own program's IDL handler
//! (processor/idl.rs) directly, since arch-cli can't reach the node/auth.
//!
//! Must run AFTER the program ELF that includes `processor/idl.rs` is live —
//! and IMMEDIATELY after that deploy/upgrade: `idl_create_account` authorizes
//! on `is_signer` alone and the account address is publicly derivable, so
//! whoever lands Create first becomes its permanent authority.
//!
//! The flow itself lives in `idl_deploy::publish_idl`, shared with
//! `dry_run_upgrade` so the dry run exercises the same code the live run will.
//!
//! Defaults preserve the historical local-testnet invocation. CI overrides via
//! env (same names as `_autara-action.yml`):
//!
//!   AUTHORITY_KEY_PATH   IDL authority + fee payer (default: keys/autara-admin-stage.key)
//!   ARCH_RPC_URL         Arch JSON-RPC (default: https://rpc.testnet.arch.network)
//!   NETWORK              testnet | testnet4 | mainnet | bitcoin (default: testnet4)
//!   IDL_PATH             path to the IDL JSON (default: idl/autara_lending.idl.json)
//!
//! Flags:
//!   --dry-run   print program id + derived IDL account; send nothing
//!   --fund      faucet-top the authority (testnet only; refused on mainnet)
//!
//! Run from repo root:  cargo run -p autara-client --bin publish_idl

use std::fs;
use std::path::PathBuf;

use arch_sdk::{
    arch_program::bitcoin::Network, with_secret_key_file, ArchRpcClient, AsyncArchRpcClient, Config,
};
use autara_client::config::path_from_workspace;
use autara_client::idl_deploy::{derive_idl_account, publish_idl};

const DEFAULT_AUTHORITY_KEY: &str = "keys/autara-admin-stage.key";
const DEFAULT_IDL_PATH: &str = "idl/autara_lending.idl.json";
const DEFAULT_RPC: &str = "https://rpc.testnet.arch.network";

fn de<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn parse_network(s: &str) -> anyhow::Result<Network> {
    // Match autara-deploy: CI's NETWORK=testnet signs as Testnet4.
    Ok(match s.to_lowercase().as_str() {
        "mainnet" | "bitcoin" => Network::Bitcoin,
        "testnet" | "testnet4" | "devnet" => Network::Testnet4,
        "regtest" | "localnet" => Network::Regtest,
        other => anyhow::bail!("unknown NETWORK '{other}' (expected testnet|testnet4|mainnet)"),
    })
}

fn authority_key_path() -> String {
    match std::env::var("AUTHORITY_KEY_PATH") {
        Ok(p) if !p.is_empty() => p,
        _ => path_from_workspace(DEFAULT_AUTHORITY_KEY),
    }
}

fn idl_path() -> PathBuf {
    match std::env::var("IDL_PATH") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(path_from_workspace(DEFAULT_IDL_PATH)),
    }
}

fn config(network: Network, rpc: String) -> Config {
    Config {
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        arch_node_url: rpc,
        titan_url: String::new(),
    }
}

fn main() -> anyhow::Result<()> {
    let dry_run = std::env::args().any(|a| a == "--dry-run");
    let fund = std::env::args().any(|a| a == "--fund");

    let network = parse_network(&env_or("NETWORK", "testnet4"))?;
    let rpc = env_or("ARCH_RPC_URL", DEFAULT_RPC);
    let config = config(network, rpc);
    let program_id = autara_program::id();
    let auth_path = authority_key_path();
    let (authority_keypair, authority_pubkey) =
        with_secret_key_file(&auth_path).map_err(|e| anyhow::anyhow!("{auth_path:?}: {e}"))?;

    let (idl_account, _base) = derive_idl_account(&program_id).map_err(de)?;

    println!("program_id (hex):    {}", hex::encode(program_id.0));
    println!(
        "program_id (base58): {}",
        bs58::encode(program_id.0).into_string()
    );
    println!(
        "idl_account (hex):    {}",
        hex::encode(idl_account.serialize())
    );
    println!(
        "idl_account (base58): {}",
        bs58::encode(idl_account.0).into_string()
    );
    println!(
        "authority (hex):      {}",
        hex::encode(authority_pubkey.serialize())
    );

    if dry_run {
        println!("DRY RUN: no IDL transactions will be sent.");
        return Ok(());
    }

    // Stamp `address` from the compiled-in id rather than trusting the file: the
    // committed IDL may name a different network's program.
    let idl_file = idl_path();
    let idl_json = {
        let mut idl: serde_json::Value = serde_json::from_slice(
            &fs::read(&idl_file).map_err(|e| anyhow::anyhow!("{idl_file:?}: {e}"))?,
        )?;
        let declared = idl.get("address").and_then(|a| a.as_str()).unwrap_or("");
        let actual = hex::encode(program_id.0);
        if declared != actual {
            println!("  rewriting IDL address {declared} -> {actual}");
        }
        idl["address"] = serde_json::Value::String(actual);
        serde_json::to_vec_pretty(&idl)?
    };

    if fund {
        anyhow::ensure!(
            !matches!(network, Network::Bitcoin),
            "refusing --fund on mainnet (no faucet)"
        );
        println!("--fund: topping up authority via faucet...");
        let sync_client = ArchRpcClient::new(&config);
        for _ in 0..3 {
            sync_client
                .create_and_fund_account_with_faucet(&authority_keypair)
                .map_err(|e| anyhow::anyhow!("faucet funding failed: {e}"))?;
        }
    }

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let client = AsyncArchRpcClient::new(&config);
            publish_idl(
                &client,
                config.network,
                program_id,
                authority_keypair,
                &idl_json,
            )
            .await
        })?;
    Ok(())
}
