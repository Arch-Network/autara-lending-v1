//! Dry-run for `upgrade_program` — validates the in-place upgrade flow against the
//! testnet node on a THROWAWAY program id, so nothing touches the live lending
//! program (6eQ1…).
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
//! Note: it writes an ELF twice (the small one, then the full ~772 KB program),
//! so it sends several hundred transactions and takes a few minutes. The
//! throwaway program is abandoned on testnet afterward; its id is printed.

use std::fs;
use std::io::Read as _;

use arch_sdk::{generate_new_keypair, ArchRpcClient, AsyncArchRpcClient, Config};
use autara_client::{
    config::path_from_workspace,
    idl_deploy::{publish_idl, upgrade_in_place},
};
use autara_deploy::elf_upload::deploy_program_elf;
use flate2::read::ZlibDecoder;

// Fresh-deploy a SMALL program, then upgrade to the big one, so the upgrade
// GROWS the account and exercises the [2/4] resize path the live run will hit
// (measured: 125 KB -> 772 KB here; live: 646 KB -> 772 KB).
const FRESH_ELF: &str = "target/deploy/autara_oracle.so";
const UPGRADE_ELF: &str = "target/deploy/autara_program.so";
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

    // 2. Fresh deploy via the deploy crate's network-safe uploader (create +
    //    write + deploy, chunks sized for the 1232-byte tx limit — 0.6.2's
    //    ProgramDeployer chunks for 10 KiB and the node rejects those txs).
    //    If THIS succeeds, the node accepts writes — the biggest unknown is cleared.
    println!("fresh-deploying SMALL ELF to throwaway (tests node accepts writes)...");
    deploy_program_elf(
        &config,
        "dry-run-autara",
        program_keypair,
        authority_keypair,
        std::path::Path::new(&fresh_elf_path),
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
        .block_on(async {
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

    // 4. Publish the IDL to the throwaway, exercising the on-chain IDL handler
    //    (Create + chunked Write, under the base-PDA signature) before it is
    //    ever driven against the live program. Nothing else covers it: the
    //    integration suite runs against the LIVE testnet deployment, so testing
    //    these sub-ops there would mutate the real IDL account.
    println!("publishing IDL to the throwaway (tests the on-chain IDL handler)...");
    let idl_json = fs::read(path_from_workspace(IDL_PATH))?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let client = AsyncArchRpcClient::new(&config);
            let idl_account = publish_idl(
                &client,
                config.network,
                program_pubkey,
                authority_keypair,
                &idl_json,
            )
            .await?;
            // Round-trip it the way the indexer does: inflate and parse.
            let acc = client
                .read_account_info(idl_account)
                .await
                .map_err(|e| anyhow::anyhow!("read idl account: {e}"))?;
            let declared = u32::from_le_bytes(acc.data[40..44].try_into().unwrap()) as usize;
            let mut json = Vec::new();
            ZlibDecoder::new(&acc.data[44..44 + declared]).read_to_end(&mut json)?;
            let parsed: serde_json::Value = serde_json::from_slice(&json)?;
            let n = parsed["instructions"].as_array().map_or(0, |a| a.len());
            println!("✓ IDL round-tripped from chain: {n} instructions decoded");
            anyhow::ensure!(n == 22, "expected 22 instructions, inflated {n}");
            Ok::<_, anyhow::Error>(())
        })?;

    println!("✓ DRY RUN PASSED — node accepts writes, upgrade flow works, IDL handler works.");
    println!(
        "  throwaway program {} is left on testnet; abandon it.",
        bs58::encode(program_pubkey.0).into_string()
    );
    Ok(())
}
