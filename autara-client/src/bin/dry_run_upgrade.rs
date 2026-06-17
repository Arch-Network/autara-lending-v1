//! Dry-run for Phase 4 — validates the upgrade flow against the testnet node on a
//! THROWAWAY program id, so nothing touches the live lending program (6eQ1…).
//!
//! It does two things:
//!   1. Fresh-deploy autara_program.so to a freshly generated program id using the
//!      proven 0.6.2 `ProgramDeployer`. This confirms the #1 unknown — that the
//!      node actually accepts `send_transaction` writes — plus create/write/deploy.
//!   2. Run the SAME `idl_deploy::upgrade_in_place` (retract → resize → write →
//!      deploy) against that throwaway, exercising the exact code that will later
//!      run on the live program.
//!
//! Run from repo root:  cargo run -p autara-client --bin dry_run_upgrade
//!
//! Note: writes the full ~616 KB ELF twice (deploy + upgrade), so it sends a lot
//! of transactions and takes a while. The throwaway program is abandoned on
//! testnet afterward; its id is printed.

use std::fs;

use arch_sdk::{
    generate_new_keypair, ArchRpcClient, AsyncArchRpcClient, Config, ProgramDeployer,
};
use autara_client::{config::path_from_workspace, idl_deploy::upgrade_in_place};

// Fresh-deploy a SMALL program, then upgrade to the big one, so the upgrade
// GROWS the account and exercises the [2/4] resize path the live run will hit
// (live: 616 KB -> 722 KB).
const FRESH_ELF: &str = "target/deploy/autara_oracle.so";
const UPGRADE_ELF: &str = "target/deploy/autara_program.so";

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

fn main() -> anyhow::Result<()> {
    let config = config();

    // Throwaway program + authority keypairs (Keypair is Copy, so we can reuse
    // them after passing into the deployer).
    let (program_keypair, program_pubkey, _) = generate_new_keypair(config.network);
    let (authority_keypair, authority_pubkey, _) = generate_new_keypair(config.network);
    println!(
        "throwaway program id: {}",
        bs58::encode(program_pubkey.0).into_string()
    );
    println!(
        "throwaway authority:  {}",
        bs58::encode(authority_pubkey.0).into_string()
    );

    let fresh_elf_path = path_from_workspace(FRESH_ELF);
    let upgrade_elf = fs::read(path_from_workspace(UPGRADE_ELF))?;

    // 1. Fund the throwaway authority via faucet (sync), like test.rs::deploy_program.
    let sync_client = ArchRpcClient::new(&config);
    println!("funding throwaway authority via faucet...");
    for _ in 0..2 {
        sync_client
            .create_and_fund_account_with_faucet(&authority_keypair)
            .map_err(|e| anyhow::anyhow!("faucet funding failed: {e}"))?;
    }

    // 2. Fresh deploy via the proven 0.6.2 ProgramDeployer (create + write + deploy).
    //    If THIS succeeds, the node accepts writes — the biggest unknown is cleared.
    println!("fresh-deploying SMALL ELF to throwaway (tests node accepts writes)...");
    ProgramDeployer::new(&config)
        .try_deploy_program(
            "dry-run-autara".to_string(),
            program_keypair,
            authority_keypair,
            &fresh_elf_path,
        )
        .map_err(|e| anyhow::anyhow!("fresh deploy failed: {e:?}"))?;
    println!("✓ fresh deploy ok (node accepts writes)");

    // Top up the throwaway authority: the upgrade pass rewrites the full ELF
    // again (writes + rent), which the initial faucet funding won't cover.
    println!("topping up throwaway authority for the upgrade pass...");
    for _ in 0..5 {
        sync_client
            .create_and_fund_account_with_faucet(&authority_keypair)
            .map_err(|e| anyhow::anyhow!("faucet top-up failed: {e}"))?;
    }

    // 3. In-place upgrade via the SAME flow used on the live program.
    println!("running in-place upgrade on throwaway (tests retract → write → deploy)...");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let client = AsyncArchRpcClient::new(&config);
            upgrade_in_place(
                &client,
                config.network,
                program_keypair,
                authority_keypair,
                &upgrade_elf,
            )
            .await
        })?;

    println!("✓ DRY RUN PASSED — node accepts writes and the full upgrade flow works.");
    println!(
        "  throwaway program {} is left on testnet; abandon it.",
        bs58::encode(program_pubkey.0).into_string()
    );
    Ok(())
}
