//! Program ELF upload that respects the Arch network transaction size limit.
//!
//! Stock `arch_sdk` 0.6.2 `ProgramDeployer` sizes loader `Write` chunks against
//! `RUNTIME_TX_SIZE_LIMIT = 10240`. Refreshed Arch testnet (and `arch_sdk`
//! 0.7.0) enforce **1232** bytes. Chunks built for 10 KiB are rejected with
//! errors like `Transaction size 10016 exceeds limit 1232`.
//!
//! We stay on workspace `arch_sdk` 0.6.2 (avoids an AsyncArchRpcClient →
//! ArchRpcClient migration) and port only the 0.7.0 chunk-sizing algorithm
//! plus the deploy/write flow.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use arch_program::bitcoin::key::Keypair;
use arch_program::bpf_loader::{LoaderState, BPF_LOADER_ID};
use arch_program::hash::Hash;
use arch_program::loader_instruction;
use arch_program::pubkey::Pubkey;
use arch_program::system_instruction;
use arch_sdk::arch_program::sanitized::ArchMessage;
use arch_sdk::{
    build_and_sign_transaction, sign_message_bip322, ArchRpcClient, Config, RuntimeTransaction,
    Signature, Status, MAX_TX_BATCH_SIZE,
};

/// Network-enforced serialized runtime transaction size (Arch testnet / sdk 0.7+).
pub const NETWORK_TX_SIZE_LIMIT: usize = 1_232;

/// Largest loader `Write` payload that still fits in a runtime transaction under
/// [`NETWORK_TX_SIZE_LIMIT`]. Matches `arch_sdk` 0.7.0 `extend_bytes_max_len`:
/// empty write payload + two distinct account keys (not the 0.6.2 probe that
/// used `system_program` twice and a 256-byte dummy buffer).
pub fn elf_write_chunk_max_len() -> usize {
    let program_pubkey = Pubkey::from([1_u8; 32]);
    let authority_pubkey = Pubkey::from([2_u8; 32]);
    let message = ArchMessage::new(
        &[loader_instruction::write(
            program_pubkey,
            authority_pubkey,
            0,
            Vec::new(),
        )],
        None,
        Hash::from([0; 32]),
    );

    let overhead = RuntimeTransaction {
        version: 0,
        signatures: vec![Signature([0_u8; 64])],
        message,
    }
    .serialize()
    .len();

    NETWORK_TX_SIZE_LIMIT
        .checked_sub(overhead)
        .expect("write-tx overhead must fit under NETWORK_TX_SIZE_LIMIT")
}

/// Deploy (or upgrade) a program ELF using network-safe write chunking.
///
/// Synchronous (uses `ArchRpcClient`); call outside an active tokio runtime,
/// matching the previous `ProgramDeployer` usage. Idempotent when the on-chain
/// ELF already matches the local file and the account is executable.
pub fn deploy_program_elf(
    config: &Config,
    program_name: &str,
    program_keypair: Keypair,
    authority_keypair: Keypair,
    elf_path: &Path,
) -> Result<Pubkey> {
    let client = ArchRpcClient::new(config);
    let elf = fs::read(elf_path)
        .with_context(|| format!("failed to read ELF file {}", elf_path.display()))?;

    let program_pubkey = Pubkey::from_slice(&program_keypair.x_only_public_key().0.serialize());
    let authority_pubkey = Pubkey::from_slice(&authority_keypair.x_only_public_key().0.serialize());

    println!(
        "\n===== PROGRAM DEPLOYMENT {} (tx limit {NETWORK_TX_SIZE_LIMIT}, write chunk {}) =====\n",
        program_name,
        elf_write_chunk_max_len()
    );

    if let Ok(account_info) = client.read_account_info(program_pubkey) {
        println!("Step 1/3: account already exists, skipping create");
        if account_info.data.len() < LoaderState::program_data_offset() {
            println!("Account not initialized; redeploying");
        } else if account_info.data[LoaderState::program_data_offset()..] == elf {
            println!("Same program already deployed; skipping ELF upload");
            if !account_info.is_executable {
                make_program_executable(
                    &client,
                    program_pubkey,
                    authority_pubkey,
                    authority_keypair,
                )?;
            }
            return Ok(program_pubkey);
        } else {
            println!("ELF mismatch with on-chain account; redeploying");
        }
    } else {
        let recent_blockhash = client
            .get_best_finalized_block_hash()
            .map_err(|e| anyhow!("get_best_finalized_block_hash failed: {e}"))?;
        let create_account_tx = build_and_sign_transaction(
            ArchMessage::new(
                &[system_instruction::create_account(
                    &authority_pubkey,
                    &program_pubkey,
                    arch_program::rent::minimum_rent(
                        LoaderState::program_data_offset() + elf.len(),
                    ),
                    0,
                    &BPF_LOADER_ID,
                )],
                Some(authority_pubkey),
                recent_blockhash,
            ),
            vec![authority_keypair, program_keypair],
            client.config.network,
        )
        .map_err(|e| anyhow!("failed to build create_account tx: {e}"))?;

        let create_account_txid = client
            .send_transaction(create_account_tx)
            .map_err(|e| anyhow!("send create_account failed: {e}"))?;
        let tx = client
            .wait_for_processed_transaction(&create_account_txid)
            .map_err(|e| anyhow!("wait create_account failed: {e}"))?;
        if let Status::Failed(e) = tx.status {
            bail!("program account creation failed ({create_account_txid}): {e}");
        }
        println!("Step 1/3: created program account ({create_account_txid})");
    }

    write_program_elf(&client, program_keypair, authority_keypair, &elf)?;

    let after = client
        .read_account_info(program_pubkey)
        .map_err(|e| anyhow!("read program account after write failed: {e}"))?;
    if after.data.get(LoaderState::program_data_offset()..) != Some(elf.as_slice()) {
        bail!("on-chain ELF does not match local file after upload");
    }
    println!("Step 2/3: ELF written and verified ({} bytes)", elf.len());

    if after.is_executable {
        println!("Step 3/3: program already executable");
    } else {
        make_program_executable(&client, program_pubkey, authority_pubkey, authority_keypair)?;
        println!("Step 3/3: made program executable");
    }

    Ok(program_pubkey)
}

fn make_program_executable(
    client: &ArchRpcClient,
    program_pubkey: Pubkey,
    authority_pubkey: Pubkey,
    authority_keypair: Keypair,
) -> Result<()> {
    let recent_blockhash = client
        .get_best_finalized_block_hash()
        .map_err(|e| anyhow!("get_best_finalized_block_hash failed: {e}"))?;
    let tx = build_and_sign_transaction(
        ArchMessage::new(
            &[loader_instruction::deploy(program_pubkey, authority_pubkey)],
            Some(authority_pubkey),
            recent_blockhash,
        ),
        vec![authority_keypair],
        client.config.network,
    )
    .map_err(|e| anyhow!("failed to build deploy tx: {e}"))?;
    let txid = client
        .send_transaction(tx)
        .map_err(|e| anyhow!("send deploy failed: {e}"))?;
    let processed = client
        .wait_for_processed_transaction(&txid)
        .map_err(|e| anyhow!("wait deploy failed: {e}"))?;
    if let Status::Failed(e) = processed.status {
        bail!("make executable failed ({txid}): {e}");
    }
    Ok(())
}

fn write_program_elf(
    client: &ArchRpcClient,
    program_keypair: Keypair,
    authority_keypair: Keypair,
    elf: &[u8],
) -> Result<()> {
    let program_pubkey = Pubkey::from_slice(&program_keypair.x_only_public_key().0.serialize());
    let authority_pubkey = Pubkey::from_slice(&authority_keypair.x_only_public_key().0.serialize());

    let account_info = client
        .read_account_info(program_pubkey)
        .map_err(|e| anyhow!("read program account before write failed: {e}"))?;

    if account_info.is_executable {
        let recent_blockhash = client
            .get_best_finalized_block_hash()
            .map_err(|e| anyhow!("get_best_finalized_block_hash failed: {e}"))?;
        let retract_tx = build_and_sign_transaction(
            ArchMessage::new(
                &[loader_instruction::retract(
                    program_pubkey,
                    authority_pubkey,
                )],
                Some(authority_pubkey),
                recent_blockhash,
            ),
            vec![authority_keypair],
            client.config.network,
        )
        .map_err(|e| anyhow!("failed to build retract tx: {e}"))?;
        let retract_txid = client
            .send_transaction(retract_tx)
            .map_err(|e| anyhow!("send retract failed: {e}"))?;
        client
            .wait_for_processed_transaction(&retract_txid)
            .map_err(|e| anyhow!("wait retract failed: {e}"))?;
    }

    if account_info.data.len() != LoaderState::program_data_offset() + elf.len() {
        let minimum_rent =
            arch_program::rent::minimum_rent(LoaderState::program_data_offset() + elf.len());
        let missing_lamports = minimum_rent.saturating_sub(account_info.lamports);
        if missing_lamports > 0 {
            let recent_blockhash = client
                .get_best_finalized_block_hash()
                .map_err(|e| anyhow!("get_best_finalized_block_hash failed: {e}"))?;
            let transfer_tx = build_and_sign_transaction(
                ArchMessage::new(
                    &[system_instruction::transfer(
                        &authority_pubkey,
                        &program_pubkey,
                        missing_lamports,
                    )],
                    Some(authority_pubkey),
                    recent_blockhash,
                ),
                vec![authority_keypair],
                client.config.network,
            )
            .map_err(|e| anyhow!("failed to build rent transfer tx: {e}"))?;
            let transfer_txid = client
                .send_transaction(transfer_tx)
                .map_err(|e| anyhow!("send rent transfer failed: {e}"))?;
            client
                .wait_for_processed_transaction(&transfer_txid)
                .map_err(|e| anyhow!("wait rent transfer failed: {e}"))?;
        }

        let recent_blockhash = client
            .get_best_finalized_block_hash()
            .map_err(|e| anyhow!("get_best_finalized_block_hash failed: {e}"))?;
        let truncate_tx = build_and_sign_transaction(
            ArchMessage::new(
                &[loader_instruction::truncate(
                    program_pubkey,
                    authority_pubkey,
                    elf.len() as u32,
                )],
                Some(authority_pubkey),
                recent_blockhash,
            ),
            vec![program_keypair, authority_keypair],
            client.config.network,
        )
        .map_err(|e| anyhow!("failed to build truncate tx: {e}"))?;
        let truncate_txid = client
            .send_transaction(truncate_tx)
            .map_err(|e| anyhow!("send truncate failed: {e}"))?;
        let processed = client
            .wait_for_processed_transaction(&truncate_txid)
            .map_err(|e| anyhow!("wait truncate failed: {e}"))?;
        if let Status::Failed(e) = processed.status {
            bail!("truncate failed ({truncate_txid}): {e}");
        }
    }

    let recent_blockhash = client
        .get_best_finalized_block_hash()
        .map_err(|e| anyhow!("get_best_finalized_block_hash failed: {e}"))?;
    let chunk_size = elf_write_chunk_max_len();
    let txs = elf
        .chunks(chunk_size)
        .enumerate()
        .map(|(i, chunk)| {
            let offset = (i * chunk_size) as u32;
            let message = ArchMessage::new(
                &[loader_instruction::write(
                    program_pubkey,
                    authority_pubkey,
                    offset,
                    chunk.to_vec(),
                )],
                Some(authority_pubkey),
                recent_blockhash,
            );
            let digest = message.hash();
            let tx = RuntimeTransaction {
                version: 0,
                signatures: vec![Signature(sign_message_bip322(
                    &authority_keypair,
                    &digest,
                    client.config.network,
                ))],
                message,
            };
            let len = tx.serialize().len();
            if len > NETWORK_TX_SIZE_LIMIT {
                bail!(
                    "ELF write chunk at offset {offset} serializes to {len} bytes (limit {NETWORK_TX_SIZE_LIMIT})"
                );
            }
            Ok(tx)
        })
        .collect::<Result<Vec<_>>>()?;

    println!(
        "Sending {} ELF write tx(s) (chunk_size={chunk_size})…",
        txs.len()
    );

    let mut tx_ids = Vec::new();
    for batch in txs.chunks(MAX_TX_BATCH_SIZE) {
        let ids = client
            .send_transactions(batch.to_vec())
            .map_err(|e| anyhow!("send_transactions failed: {e}"))?;
        tx_ids.extend(ids);
    }

    for (i, tx_id) in tx_ids.iter().enumerate() {
        let processed = client
            .wait_for_processed_transaction(tx_id)
            .map_err(|e| anyhow!("wait write tx {tx_id} failed: {e}"))?;
        if let Status::Failed(reason) = processed.status {
            let offset = (i * chunk_size) as u32;
            bail!("ELF write failed at offset {offset} ({tx_id}): {reason}");
        }
        if (i + 1) % 50 == 0 || i + 1 == tx_ids.len() {
            println!("  confirmed {}/{}", i + 1, tx_ids.len());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_program::hash::Hash;
    use arch_program::loader_instruction;
    use arch_program::pubkey::Pubkey;
    use arch_sdk::arch_program::sanitized::ArchMessage;
    use arch_sdk::{RuntimeTransaction, Signature};

    #[test]
    fn elf_write_chunk_fills_network_tx_limit() {
        let max_len = elf_write_chunk_max_len();
        assert!(max_len > 0, "chunk size must be positive");
        assert!(
            max_len < NETWORK_TX_SIZE_LIMIT,
            "chunk payload must leave room for headers"
        );

        let program_pubkey = Pubkey::from([1_u8; 32]);
        let authority_pubkey = Pubkey::from([2_u8; 32]);
        let message = ArchMessage::new(
            &[loader_instruction::write(
                program_pubkey,
                authority_pubkey,
                0,
                vec![0_u8; max_len],
            )],
            None,
            Hash::from([0; 32]),
        );
        let tx = RuntimeTransaction {
            version: 0,
            signatures: vec![Signature([0_u8; 64])],
            message,
        };
        assert_eq!(tx.serialize().len(), NETWORK_TX_SIZE_LIMIT);
    }

    #[test]
    fn stock_0_6_2_chunk_exceeds_network_limit() {
        // Replicate arch_sdk 0.6.2 extend_bytes_max_len (10240 limit + 256 dummy).
        let message = ArchMessage::new(
            &[loader_instruction::write(
                Pubkey::system_program(),
                Pubkey::system_program(),
                0,
                vec![0_u8; 256],
            )],
            None,
            Hash::from([0; 32]),
        );
        let old_chunk = 10_240usize
            - RuntimeTransaction {
                version: 0,
                signatures: vec![Signature([0_u8; 64])],
                message,
            }
            .serialize()
            .len();

        let program_pubkey = Pubkey::from([1_u8; 32]);
        let authority_pubkey = Pubkey::from([2_u8; 32]);
        let real_message = ArchMessage::new(
            &[loader_instruction::write(
                program_pubkey,
                authority_pubkey,
                0,
                vec![0_u8; old_chunk],
            )],
            None,
            Hash::from([0; 32]),
        );
        let real_len = RuntimeTransaction {
            version: 0,
            signatures: vec![Signature([0_u8; 64])],
            message: real_message,
        }
        .serialize()
        .len();

        assert!(
            real_len > NETWORK_TX_SIZE_LIMIT,
            "expected stock 0.6.2-sized write ({real_len}) to exceed {NETWORK_TX_SIZE_LIMIT}"
        );
    }
}
