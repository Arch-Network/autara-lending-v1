//! In-place upgrade of the LIVE Autara program at the SAME program id.
//!
//! Thin wrapper over `autara_client::idl_deploy::upgrade_in_place` (the shared
//! loader retract → resize → write → deploy flow). Reuses `keys/autara-stage.key`
//! (so the id stays 6eQ1…GU5cm) and signs with `keys/autara-deployer.key`.
//!
//! Run from repo root:  cargo run -p autara-client --bin upgrade_program
//!
//! ⚠️ Mutates a LIVE, fund-holding program and is non-atomic (the program is
//! non-executable between retract and deploy). Dry-run on a throwaway first:
//!   cargo run -p autara-client --bin dry_run_upgrade

use std::fs;

use arch_sdk::{with_secret_key_file, ArchRpcClient, AsyncArchRpcClient, Config};
use autara_client::{config::path_from_workspace, idl_deploy::upgrade_in_place};

const PROGRAM_KEY: &str = "keys/autara-stage.key";
const AUTHORITY_KEY: &str = "keys/autara-deployer.key";
const ELF_PATH: &str = "target/deploy/autara_program.so";
const EXPECTED_PROGRAM_B58: &str = "6eQ1vLSAwmbT6SD3KQbNawAqis7LpzwpNTd7SJ1GU5cm";

fn config() -> Config {
    Config {
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network: arch_sdk::arch_program::bitcoin::Network::Testnet4,
        // Raw node (proven via check_program). If it rejects writes, the
        // authenticated indexer (/api/v1/rpc) would need a custom transport.
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        titan_url: String::new(),
    }
}

fn main() -> anyhow::Result<()> {
    let config = config();
    let (program_keypair, program_pubkey) =
        with_secret_key_file(&path_from_workspace(PROGRAM_KEY)).map_err(|e| anyhow::anyhow!(e))?;
    let (authority_keypair, _authority_pubkey) =
        with_secret_key_file(&path_from_workspace(AUTHORITY_KEY)).map_err(|e| anyhow::anyhow!(e))?;

    anyhow::ensure!(
        bs58::encode(program_pubkey.0).into_string() == EXPECTED_PROGRAM_B58,
        "program keypair does not match expected program id {EXPECTED_PROGRAM_B58}"
    );

    let elf = fs::read(path_from_workspace(ELF_PATH))?;
    println!("Upgrading LIVE program {EXPECTED_PROGRAM_B58} ({} bytes ELF)", elf.len());

    // Optional: top up the authority via faucet so it can cover the rent increase
    // + write/retract/deploy fees. Underfunding mid-write leaves the program
    // retracted (down), so prefer over-funding. Testnet only.
    if std::env::args().any(|a| a == "--fund") {
        println!("--fund: topping up authority via faucet...");
        let sync_client = ArchRpcClient::new(&config);
        for _ in 0..5 {
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
            upgrade_in_place(&client, config.network, program_keypair, authority_keypair, &elf).await
        })?;
    Ok(())
}
