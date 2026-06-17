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
    build_and_sign_transaction, extend_bytes_max_len, sign_message_bip322, AsyncArchRpcClient,
    RuntimeTransaction, Signature, Status, MAX_TX_BATCH_SIZE,
};

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

    // 3. write ELF in chunks (batched)
    println!("  [3/4] write ELF chunks");
    let chunk_size = extend_bytes_max_len();
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
            signatures: vec![Signature(sign_message_bip322(&authority_keypair, &digest, net))],
            message,
        });
    }
    println!("    {} write txs (chunk_size={})", txs.len(), chunk_size);
    let mut ids = Vec::new();
    for batch in txs.chunks(MAX_TX_BATCH_SIZE) {
        ids.extend(client.send_transactions(batch.to_vec()).await.map_err(de)?);
    }
    for txid in &ids {
        confirm(client, txid).await?;
    }
    if let (Some(first), Some(last)) = (ids.first(), ids.last()) {
        println!("        wrote {} chunks", ids.len());
        println!("        first write tx {}  {}", tx_b58(first), explorer_tx(first));
        println!("        last  write tx {}  {}", tx_b58(last), explorer_tx(last));
    }

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
