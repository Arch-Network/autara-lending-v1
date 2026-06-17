//! Phase 5 — publish Autara's IDL on-chain by driving our own program's IDL
//! handler (processor/idl.rs) directly, since arch-cli can't reach the node/auth.
//!
//! Flow (must run AFTER the program upgrade that adds the handler — Phase 4):
//!   1. zlib-compress idl/autara_lending.idl.json
//!   2. derive the anchor:idl account = create_with_seed(find_program_address([],id), "anchor:idl", id)
//!   3. send IdlInstruction::Create { data_len } to allocate + init the account
//!   4. send IdlInstruction::Write { data } in sequential chunks (each appends)
//! Instruction data = IDL_IX_TAG_LE (8 bytes) ++ borsh(IdlInstruction).
//!
//! Authority/payer = keys/autara-admin-stage.key (becomes the IDL authority).
//!
//! Run from repo root:  cargo run -p autara-client --bin publish_idl
//!
//! ⚠️ UNTESTED here. The IDL is small (~1.5 KB zlib) so it fits one Create
//! (space < 10 KB) + a couple of Writes — no buffer/resize path needed.

use std::fs;
use std::io::Write as _;

use arch_sdk::{
    arch_program::{
        account::AccountMeta, instruction::Instruction, pubkey::Pubkey,
        sanitized::ArchMessage, system_program::SYSTEM_PROGRAM_ID,
    },
    build_and_sign_transaction, with_secret_key_file, AsyncArchRpcClient, Config, Status,
};
use autara_client::config::path_from_workspace;
use flate2::{write::ZlibEncoder, Compression};

// Must match processor::idl. Reuse the program's own constant to avoid drift.
use autara_program::processor::idl::IDL_IX_TAG_LE;

const AUTHORITY_KEY: &str = "keys/autara-admin-stage.key";
const IDL_PATH: &str = "idl/autara_lending.idl.json";
const IDL_SEED: &str = "anchor:idl";
const WRITE_CHUNK: usize = 800; // conservative; tx-size safe for our small IDL

// IdlInstruction borsh variant tags (declaration order in processor/idl.rs).
const IX_CREATE: u8 = 0;
const IX_WRITE: u8 = 2;

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
    let (authority_keypair, authority_pubkey) =
        with_secret_key_file(&path_from_workspace(AUTHORITY_KEY)).map_err(de)?;

    // Derive the canonical IDL account (same derivation as our handler + indexer).
    let (base, _bump) = Pubkey::find_program_address(&[], &program_id);
    let idl_account = Pubkey::create_with_seed(&base, IDL_SEED, &program_id).map_err(de)?;

    // Compress the IDL JSON (zlib / RFC1950, matching the indexer's ZlibDecoder).
    let raw = fs::read(path_from_workspace(IDL_PATH))?;
    let compressed = {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw)?;
        enc.finish()?
    };
    println!(
        "Publishing IDL for {}",
        bs58::encode(program_id.0).into_string()
    );
    println!("  IDL json {} B -> zlib {} B", raw.len(), compressed.len());
    println!("  IDL account: {}", bs58::encode(idl_account.0).into_string());

    // Create instruction data: tag ++ [IX_CREATE] ++ data_len:u64 LE
    let mut create_data = IDL_IX_TAG_LE.to_vec();
    create_data.push(IX_CREATE);
    create_data.extend_from_slice(&(compressed.len() as u64).to_le_bytes());

    let create_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(authority_pubkey, true), // from (payer, signer)
            AccountMeta::new(idl_account, false),     // to (created via CPI)
            AccountMeta::new_readonly(base, false),   // base PDA
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(program_id, false), // program (handler checks == id)
        ],
        data: create_data,
    };

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let client = AsyncArchRpcClient::new(&config);
            let net = config.network;

            // 1. Create (skip if the account already exists from a previous run).
            if client.read_account_info(idl_account).await.is_err() {
                println!("  [create] allocating IDL account");
                send(&client, net, &authority_keypair, authority_pubkey, create_ix).await?;
            } else {
                println!("  [create] IDL account already exists, skipping create");
            }

            // 2. Write chunks sequentially (each appends at the current data_len).
            for (i, chunk) in compressed.chunks(WRITE_CHUNK).enumerate() {
                let mut data = IDL_IX_TAG_LE.to_vec();
                data.push(IX_WRITE);
                data.extend_from_slice(&(chunk.len() as u32).to_le_bytes()); // borsh Vec<u8> len
                data.extend_from_slice(chunk);
                let write_ix = Instruction {
                    program_id,
                    accounts: vec![
                        AccountMeta::new(idl_account, false),       // idl (writable)
                        AccountMeta::new(authority_pubkey, true),   // authority (signer)
                    ],
                    data,
                };
                println!("  [write {}] {} bytes", i, chunk.len());
                send(&client, net, &authority_keypair, authority_pubkey, write_ix).await?;
            }

            // 3. Verify: header data_len field == compressed length.
            let acc = client.read_account_info(idl_account).await.map_err(de)?;
            anyhow::ensure!(acc.data.len() >= 44, "idl account too small");
            let data_len = u32::from_le_bytes(acc.data[40..44].try_into().unwrap()) as usize;
            anyhow::ensure!(
                data_len == compressed.len(),
                "on-chain data_len {} != compressed {}",
                data_len,
                compressed.len()
            );
            println!("✓ IDL published. Verify with the indexer's IDL fetch / explorer decode.");
            Ok::<_, anyhow::Error>(())
        })?;
    Ok(())
}

async fn send(
    client: &AsyncArchRpcClient,
    net: arch_sdk::arch_program::bitcoin::Network,
    authority_keypair: &arch_sdk::arch_program::bitcoin::key::Keypair,
    authority_pubkey: Pubkey,
    ix: Instruction,
) -> anyhow::Result<()> {
    let bh = client.get_best_finalized_block_hash().await.map_err(de)?;
    let tx = build_and_sign_transaction(
        ArchMessage::new(&[ix], Some(authority_pubkey), bh),
        vec![*authority_keypair],
        net,
    )
    .map_err(de)?;
    let txid = client.send_transaction(tx).await.map_err(de)?;
    let processed = client
        .wait_for_processed_transaction(&txid)
        .await
        .map_err(de)?;
    if let Status::Failed(reason) = processed.status {
        anyhow::bail!("tx {txid} failed: {reason}");
    }
    Ok(())
}
