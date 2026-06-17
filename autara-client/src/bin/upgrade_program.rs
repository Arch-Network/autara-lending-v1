//! Phase 4 — in-place upgrade of the Autara program at the SAME program id.
//!
//! Why this exists: arch-cli 0.6.5 can't talk to the testnet node (version
//! mismatch) and can't auth to the indexer; the pinned `arch_sdk = 0.6.2`
//! `ProgramDeployer` is fresh-deploy-only (errors on "already exists"). So this
//! bin drives the loader-v4 upgrade lifecycle directly:
//!     retract -> (resize: transfer rent + truncate) -> write ELF chunks -> deploy
//! mirroring `arch_sdk` helper `async_program_deployment.rs::write_program_elf`.
//!
//! It reuses the program keypair (`keys/autara-stage.key`) so the id stays
//! 6eQ1…GU5cm, and signs management instructions with the upgrade authority
//! (`keys/autara-deployer.key`) — both verified by `check_program`.
//!
//! Run from repo root:  cargo run -p autara-client --bin upgrade_program
//!
//! ⚠️ UNTESTED in this environment and it mutates a LIVE, fund-holding program.
//! Dry-run on a throwaway program id first if at all possible. VERIFY markers
//! note 0.6.2 API points to confirm at compile time.

use std::fs;

use arch_sdk::{
    arch_program::{
        bpf_loader::{LoaderState, BPF_LOADER_ID},
        loader_instruction,
        pubkey::Pubkey,
        rent::minimum_rent,
        sanitized::ArchMessage,
        system_instruction,
    },
    build_and_sign_transaction, extend_bytes_max_len, sign_message_bip322, with_secret_key_file,
    AsyncArchRpcClient, Config, RuntimeTransaction, Signature, Status, MAX_TX_BATCH_SIZE,
};
use autara_client::config::path_from_workspace;

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
        // Default: raw node (proven to work with the 0.6.2 stack via check_program).
        // If the raw node rejects writes, the fallback is the authenticated indexer
        // (https://explorer.arch.network/api/v1/rpc) — but arch_sdk can't send the
        // API-key header, so that path needs a custom transport (not this bin).
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        titan_url: String::new(),
    }
}

fn main() -> anyhow::Result<()> {
    let config = config();
    let (program_keypair, program_pubkey) =
        with_secret_key_file(&path_from_workspace(PROGRAM_KEY)).map_err(|e| anyhow::anyhow!(e))?;
    let (authority_keypair, authority_pubkey) =
        with_secret_key_file(&path_from_workspace(AUTHORITY_KEY)).map_err(|e| anyhow::anyhow!(e))?;

    anyhow::ensure!(
        bs58::encode(program_pubkey.0).into_string() == EXPECTED_PROGRAM_B58,
        "program keypair does not match expected program id {EXPECTED_PROGRAM_B58}"
    );

    let elf = fs::read(path_from_workspace(ELF_PATH))?;
    println!(
        "Upgrading program {} ({} bytes ELF)",
        EXPECTED_PROGRAM_B58,
        elf.len()
    );

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let client = AsyncArchRpcClient::new(&config);
            let net = config.network;

            // --- preflight: program must exist, be executable, and we hold authority
            let acc = client
                .read_account_info(program_pubkey)
                .await
                .map_err(|e| anyhow::anyhow!("read program account: {e}"))?;
            anyhow::ensure!(
                bs58::encode(acc.owner.0).into_string()
                    == bs58::encode(BPF_LOADER_ID.0).into_string(),
                "program not owned by the BPF loader"
            );
            anyhow::ensure!(
                acc.data.len() >= LoaderState::program_data_offset(),
                "program account too small to hold a loader header"
            );
            let onchain_authority = Pubkey::from_slice(&acc.data[0..32]);
            anyhow::ensure!(
                onchain_authority == authority_pubkey,
                "on-chain upgrade authority != keys/autara-deployer.key"
            );
            println!("  preflight ok: executable={}", acc.is_executable);

            // --- 1. retract (only if currently executable)
            if acc.is_executable {
                println!("  [1/4] retract");
                let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
                let tx = build_and_sign_transaction(
                    ArchMessage::new(
                        &[loader_instruction::retract(program_pubkey, authority_pubkey)],
                        Some(authority_pubkey),
                        bh,
                    ),
                    vec![authority_keypair],
                    net,
                )
                .map_err(de)?;
                let txid = client.send_transaction(tx).await.map_err(de)?;
                confirm(&client, &txid).await?;
            }

            // --- 2. resize to the new ELF size (transfer missing rent, then truncate)
            let needed = LoaderState::program_data_offset() + elf.len();
            if acc.data.len() != needed {
                println!(
                    "  [2/4] resize {} -> {} bytes",
                    acc.data.len(),
                    needed
                );
                let want_lamports = minimum_rent(needed);
                let missing = want_lamports.saturating_sub(acc.lamports);
                if missing > 0 {
                    let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
                    let tx = build_and_sign_transaction(
                        ArchMessage::new(
                            &[system_instruction::transfer(
                                &authority_pubkey,
                                &program_pubkey,
                                missing,
                            )],
                            Some(authority_pubkey),
                            bh,
                        ),
                        vec![authority_keypair],
                        net,
                    )
                    .map_err(de)?;
                    let txid = client.send_transaction(tx).await.map_err(de)?;
                    confirm(&client, &txid).await?;
                }
                let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
                let tx = build_and_sign_transaction(
                    ArchMessage::new(
                        &[loader_instruction::truncate(
                            program_pubkey,
                            authority_pubkey,
                            elf.len() as u32,
                        )],
                        Some(authority_pubkey),
                        bh,
                    ),
                    // truncate is signed by BOTH program + authority (matches SDK).
                    vec![program_keypair, authority_keypair],
                    net,
                )
                .map_err(de)?;
                let txid = client.send_transaction(tx).await.map_err(de)?;
                confirm(&client, &txid).await?;
            }

            // --- 3. write ELF in chunks (batched), mirroring SDK send_elf_chunks
            println!("  [3/4] write ELF chunks");
            let chunk_size = extend_bytes_max_len(); // VERIFY exists in arch_sdk 0.6.2
            let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
            let mut txs: Vec<RuntimeTransaction> = Vec::new();
            for (i, chunk) in elf.chunks(chunk_size).enumerate() {
                let offset = (i * chunk_size) as u32;
                let message = ArchMessage::new(
                    &[loader_instruction::write(
                        program_pubkey,
                        authority_pubkey,
                        offset,
                        chunk.to_vec(),
                    )],
                    Some(authority_pubkey),
                    bh,
                );
                let digest = message.hash();
                txs.push(RuntimeTransaction {
                    version: 0,
                    signatures: vec![Signature(sign_message_bip322(
                        &authority_keypair,
                        &digest,
                        net,
                    ))],
                    message,
                });
            }
            println!("    {} write txs (chunk_size={})", txs.len(), chunk_size);
            let mut ids = Vec::new();
            for batch in txs.chunks(MAX_TX_BATCH_SIZE) {
                ids.extend(client.send_transactions(batch.to_vec()).await.map_err(de)?);
            }
            for txid in &ids {
                confirm(&client, txid).await?;
            }

            // --- 4. deploy (make executable again)
            println!("  [4/4] deploy");
            let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
            let tx = build_and_sign_transaction(
                ArchMessage::new(
                    &[loader_instruction::deploy(program_pubkey, authority_pubkey)],
                    Some(authority_pubkey),
                    bh,
                ),
                vec![authority_keypair],
                net,
            )
            .map_err(de)?;
            let txid = client.send_transaction(tx).await.map_err(de)?;
            confirm(&client, &txid).await?;

            // --- verify
            let acc = client.read_account_info(program_pubkey).await.map_err(de)?;
            anyhow::ensure!(acc.is_executable, "program not executable after deploy");
            anyhow::ensure!(
                acc.data[LoaderState::program_data_offset()..] == elf[..],
                "on-chain ELF does not match local file after upgrade"
            );
            println!("✓ upgrade complete; program id unchanged, ELF verified.");
            Ok::<_, anyhow::Error>(())
        })?;
    Ok(())
}

fn de<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

async fn confirm(client: &AsyncArchRpcClient, txid: &arch_sdk::arch_program::hash::Hash) -> anyhow::Result<()> {
    let tx = client
        .wait_for_processed_transaction(txid)
        .await
        .map_err(de)?;
    if let Status::Failed(reason) = tx.status {
        anyhow::bail!("tx {txid} failed: {reason}");
    }
    Ok(())
}
