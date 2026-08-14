//! Publish Autara's IDL on-chain by driving our own program's IDL handler
//! (processor/idl.rs) directly, since arch-cli can't reach the node/auth.
//!
//! Must run AFTER `upgrade_program` — the handler only exists on chain once the
//! new ELF is deployed — and IMMEDIATELY after it: `idl_create_account`
//! authorizes on `is_signer` alone and the account address is publicly
//! derivable, so whoever lands Create first becomes its permanent authority.
//!
//! The flow itself lives in `idl_deploy::publish_idl`, shared with
//! `dry_run_upgrade` so it is exercised against a throwaway program before it
//! ever runs here.
//!
//! Authority/payer = keys/autara-admin-stage.key (becomes the IDL authority).
//!
//! Run from repo root:  cargo run -p autara-client --bin publish_idl

use std::fs;

use arch_sdk::{with_secret_key_file, AsyncArchRpcClient, Config};
use autara_client::config::path_from_workspace;
use autara_client::idl_deploy::publish_idl;

const AUTHORITY_KEY: &str = "keys/autara-admin-stage.key";
const IDL_PATH: &str = "idl/autara_lending.idl.json";

fn config() -> Config {
    Config {
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network: arch_sdk::arch_program::bitcoin::Network::Testnet4,
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        titan_url: String::new(),
    }
}

fn de<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn main() -> anyhow::Result<()> {
    let config = config();
    let program_id = autara_program::id();
    let (authority_keypair, _authority_pubkey) =
        with_secret_key_file(&path_from_workspace(AUTHORITY_KEY)).map_err(de)?;

    // Stamp `address` from the compiled-in id rather than trusting the file: the
    // committed IDL names the testnet program, so publishing from a mainnet
    // build would otherwise upload an IDL declaring the wrong program.
    let idl_json = {
        let mut idl: serde_json::Value =
            serde_json::from_slice(&fs::read(path_from_workspace(IDL_PATH))?)?;
        let declared = idl.get("address").and_then(|a| a.as_str()).unwrap_or("");
        let actual = hex::encode(program_id.0);
        if declared != actual {
            println!("  rewriting IDL address {declared} -> {actual}");
        }
        idl["address"] = serde_json::Value::String(actual);
        serde_json::to_vec_pretty(&idl)?
    };

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
