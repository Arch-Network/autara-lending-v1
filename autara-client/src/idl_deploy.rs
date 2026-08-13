//! Shared in-place program-upgrade flow (loader-v4 retract → resize → write →
//! deploy), used by both `bin/upgrade_program` (live program) and
//! `bin/dry_run_upgrade` (throwaway program) so the dry-run exercises the same
//! code as the live upgrade.
//!
//! Mirrors `arch_sdk` helper `async_program_deployment.rs::write_program_elf`,
//! reimplemented on the pinned 0.6.2 API because the 0.6.2 `ProgramDeployer` is
//! fresh-deploy-only.

use arch_sdk::{
    arch_program::{
        bitcoin::{key::Keypair, Network},
        bpf_loader::{LoaderState, BPF_LOADER_ID},
        hash::Hash,
        loader_instruction,
        pubkey::Pubkey,
        rent::minimum_rent,
        sanitized::ArchMessage,
        system_instruction,
    },
    build_and_sign_transaction, sign_message_bip322, AsyncArchRpcClient, RuntimeTransaction,
    Signature, Status, MAX_TX_BATCH_SIZE,
};
// arch_sdk 0.6.2's `extend_bytes_max_len` sizes chunks for the old 10 KiB tx
// limit; refreshed testnet enforces 1232 bytes. Use the 0.7.0-style probe.
use autara_deploy::elf_upload::elf_write_chunk_max_len;

fn de<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// Base58 of a tx hash (matches how the explorer displays transaction ids; the
/// `Hash` Display impl is hex, which the explorer doesn't use in its URLs).
pub fn tx_b58(txid: &Hash) -> String {
    bs58::encode(&txid.as_ref()[..]).into_string()
}

/// Explorer URL for a transaction (testnet). Verify the path against your
/// explorer if it ever 404s — the base58 id above also works in the search box.
pub fn explorer_tx(txid: &Hash) -> String {
    format!(
        "https://explorer.arch.network/testnet/transactions/{}",
        tx_b58(txid)
    )
}

/// How many write/reconcile passes before giving up (each pass re-signs the
/// still-missing chunks against a fresh blockhash).
const MAX_WRITE_PASSES: usize = 4;

/// Seconds between readbacks while waiting for a pass's writes to land.
const WRITE_POLL_SECS: u64 = 15;

/// Consecutive readbacks showing no progress before a pass gives up and re-sends
/// what is still missing. Only a stall ends a pass — as long as chunks keep
/// arriving we keep waiting, however long that takes.
const WRITE_STALL_LIMIT: u32 = 4;

/// Indices of the chunks whose on-chain bytes do not match `elf`.
///
/// `account_data` is the raw program account, i.e. the loader header followed by
/// the ELF; anything short of the full expected length counts as missing.
fn missing_chunks(account_data: &[u8], elf: &[u8], chunk_size: usize) -> Vec<usize> {
    let onchain = account_data
        .get(LoaderState::program_data_offset()..)
        .unwrap_or_default();
    elf.chunks(chunk_size)
        .enumerate()
        .filter(|(i, want)| {
            let start = i * chunk_size;
            onchain
                .get(start..start + want.len())
                .is_none_or(|got| got != *want)
        })
        .map(|(i, _)| i)
        .collect()
}

async fn confirm(client: &AsyncArchRpcClient, txid: &Hash) -> anyhow::Result<()> {
    let tx = client.wait_for_processed_transaction(txid).await.map_err(de)?;
    if let Status::Failed(reason) = tx.status {
        anyhow::bail!("tx {txid} failed: {reason}");
    }
    Ok(())
}

/// Send + confirm a single tx and print its id + explorer URL under `label`.
async fn send_and_log(
    client: &AsyncArchRpcClient,
    tx: RuntimeTransaction,
    label: &str,
) -> anyhow::Result<()> {
    let txid = client.send_transaction(tx).await.map_err(de)?;
    confirm(client, &txid).await?;
    println!("        {label} tx {}  {}", tx_b58(&txid), explorer_tx(&txid));
    Ok(())
}

/// Upgrade an already-deployed, executable program in place at the same id.
///
/// Preconditions (checked here): the program account exists, is owned by the BPF
/// loader, and its on-chain authority equals `authority_keypair`. Performs
/// `retract → (resize) → write chunks → deploy`, then verifies the on-chain ELF.
pub async fn upgrade_in_place(
    client: &AsyncArchRpcClient,
    net: Network,
    program_keypair: Keypair,
    authority_keypair: Keypair,
    elf: &[u8],
) -> anyhow::Result<()> {
    let program_pubkey = Pubkey::from_slice(&program_keypair.x_only_public_key().0.serialize());
    let authority_pubkey = Pubkey::from_slice(&authority_keypair.x_only_public_key().0.serialize());

    let acc = client
        .read_account_info(program_pubkey)
        .await
        .map_err(|e| anyhow::anyhow!("read program account: {e}"))?;
    anyhow::ensure!(
        bs58::encode(acc.owner.0).into_string() == bs58::encode(BPF_LOADER_ID.0).into_string(),
        "program not owned by the BPF loader"
    );
    anyhow::ensure!(
        acc.data.len() >= LoaderState::program_data_offset(),
        "program account too small to hold a loader header"
    );
    let onchain_authority = Pubkey::from_slice(&acc.data[0..32]);
    anyhow::ensure!(
        onchain_authority == authority_pubkey,
        "on-chain upgrade authority != provided authority keypair"
    );
    println!("  preflight ok: executable={}", acc.is_executable);

    // 1. retract (only if currently executable)
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
        send_and_log(client, tx, "retract").await?;
    }

    // 2. resize to new ELF size (transfer missing rent, then truncate)
    let needed = LoaderState::program_data_offset() + elf.len();
    if acc.data.len() != needed {
        println!("  [2/4] resize {} -> {} bytes", acc.data.len(), needed);
        let missing = minimum_rent(needed).saturating_sub(acc.lamports);
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
            send_and_log(client, tx, "rent-transfer").await?;
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
            vec![program_keypair, authority_keypair],
            net,
        )
        .map_err(de)?;
        send_and_log(client, tx, "truncate").await?;
    }

    // 3. write ELF in chunks, then reconcile against the chain and rewrite gaps.
    //
    // Writes are sent optimistically and never individually confirmed: the
    // authority on whether a chunk landed is the program account itself. Each
    // pass re-reads it, diffs against the local ELF, and rewrites only the
    // chunks that are still wrong. That is what makes this safe against the
    // three ways a write silently disappears here:
    //   * a lost HTTP response (the pool may still hold the txs) — resending the
    //     identical batch would be rejected wholesale with "transaction already
    //     exists in pool", so a repair pass signs against a FRESH blockhash and
    //     is therefore a different transaction, not a duplicate;
    //   * blockhash expiry — txs older than DEFAULT_SIGNATURE_VALIDITY_BLOCKS
    //     (150) are dropped at propose time with no processed-transaction
    //     record, so per-tx confirmation would just block until it timed out;
    //   * a partially-accepted batch.
    // Verification happens BEFORE deploy, so a short write can never be made
    // executable.
    println!("  [3/4] write ELF chunks");
    let chunk_size = elf_write_chunk_max_len();
    let total_chunks = elf.chunks(chunk_size).count();
    // Seed from what is actually on chain rather than assuming everything is
    // missing: a re-run after an aborted upgrade then rewrites only the gaps,
    // and any chunk the previous ELF already happens to match is skipped.
    let acc = client.read_account_info(program_pubkey).await.map_err(de)?;
    let mut todo = missing_chunks(&acc.data, elf, chunk_size);
    println!("    {} / {total_chunks} chunk(s) to write", todo.len());

    for pass in 1..=MAX_WRITE_PASSES {
        let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
        let txs: Vec<RuntimeTransaction> = todo
            .iter()
            .map(|&i| {
                let offset = (i * chunk_size) as u32;
                let message = ArchMessage::new(
                    &[loader_instruction::write(
                        program_pubkey,
                        authority_pubkey,
                        offset,
                        elf[i * chunk_size..((i + 1) * chunk_size).min(elf.len())].to_vec(),
                    )],
                    Some(authority_pubkey),
                    bh,
                );
                let digest = message.hash();
                RuntimeTransaction {
                    version: 0,
                    signatures: vec![Signature(sign_message_bip322(
                        &authority_keypair,
                        &digest,
                        net,
                    ))],
                    message,
                }
            })
            .collect();
        println!(
            "    pass {pass}: {} write tx(s) (chunk_size={chunk_size})",
            txs.len()
        );

        // Send errors are logged, not fatal: the batch may well have landed, and
        // the readback below is what decides.
        for batch in txs.chunks(MAX_TX_BATCH_SIZE) {
            if let Err(e) = client.send_transactions(batch.to_vec()).await.map_err(de) {
                println!("        send batch failed (will reconcile): {e}");
            }
        }

        // Wait for the pass's writes to land. Give up only once progress STALLS,
        // not on a fixed deadline: several hundred chunks take minutes to settle,
        // and resending chunks that are merely still in flight wastes a whole
        // pass (and fees) rewriting bytes that were about to arrive anyway.
        let mut stalls = 0;
        while stalls < WRITE_STALL_LIMIT {
            tokio::time::sleep(std::time::Duration::from_secs(WRITE_POLL_SECS)).await;
            let acc = client.read_account_info(program_pubkey).await.map_err(de)?;
            let remaining = missing_chunks(&acc.data, elf, chunk_size);
            stalls = if remaining.len() < todo.len() {
                0
            } else {
                stalls + 1
            };
            todo = remaining;
            println!("        {} / {total_chunks} chunk(s) still missing", todo.len());
            if todo.is_empty() {
                break;
            }
        }
        if todo.is_empty() {
            break;
        }
    }
    anyhow::ensure!(
        todo.is_empty(),
        "{} of {total_chunks} ELF chunks still not written after {MAX_WRITE_PASSES} passes; \
         program is RETRACTED — re-run upgrade_program to finish (it resumes)",
        todo.len()
    );
    println!("        all {total_chunks} chunks verified on chain");

    // 4. deploy (make executable) — this is the tx that re-activates the program
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
    send_and_log(client, tx, "deploy (upgrade completion)").await?;

    // verify
    let acc = client.read_account_info(program_pubkey).await.map_err(de)?;
    anyhow::ensure!(acc.is_executable, "program not executable after deploy");
    anyhow::ensure!(
        acc.data[LoaderState::program_data_offset()..] == elf[..],
        "on-chain ELF does not match local file after upgrade"
    );
    println!("✓ upgrade complete; ELF verified.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HDR: usize = LoaderState::program_data_offset();

    fn account(elf: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; HDR];
        v.extend_from_slice(elf);
        v
    }

    #[test]
    fn fully_written_elf_has_no_missing_chunks() {
        let elf: Vec<u8> = (0..250u8).cycle().take(1000).collect();
        assert!(missing_chunks(&account(&elf), &elf, 300).is_empty());
    }

    #[test]
    fn corrupt_and_truncated_chunks_are_reported() {
        let elf: Vec<u8> = (0..250u8).cycle().take(1000).collect();

        // A single wrong byte inside chunk 1 flags only chunk 1.
        let mut corrupt = account(&elf);
        corrupt[HDR + 350] ^= 0xff;
        assert_eq!(missing_chunks(&corrupt, &elf, 300), vec![1]);

        // A short account flags every chunk past the truncation point (chunk 2
        // is partially present, so it counts as missing too).
        let short = account(&elf[..700]);
        assert_eq!(missing_chunks(&short, &elf, 300), vec![2, 3]);

        // Nothing written at all (header only) flags everything.
        assert_eq!(missing_chunks(&account(&[]), &elf, 300), vec![0, 1, 2, 3]);
    }
}
