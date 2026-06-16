//! Read-only diagnostic for the Phase 4 (in-place upgrade) preconditions.
//!
//! Answers the two questions you must confirm before upgrading the Autara
//! program on testnet:
//!   1. Is the program still **upgradeable** (not `finalize`d / immutable)?
//!   2. Is `keys/autara-deployer.key` the **current upgrade authority**?
//!
//! It also prints the program id derived from `keys/autara-stage.key` (so you can
//! confirm the keypair still matches the deployed program) and the on-chain
//! account's owner (the loader) + size.
//!
//! Run from the repo root:
//!   cargo run -p autara-client --bin check_program
//!
//! The loader header layout is Arch's `arch_program::bpf_loader::LoaderState`:
//!   [0..32]  authority_address_or_next_version: Pubkey
//!   [32..40] status: LoaderStatus  (u64 LE; 0=Retracted, 1=Deployed, 2=Finalized)
//!   [40..]   ELF  (program_data_offset = size_of::<LoaderState>() = 40)
//! On Arch, `BpfLoader1111…` IS the upgradeable loader-v4, so a program owned by
//! it can be upgraded in place (retract -> write -> deploy) while it is not Finalized.

use arch_sdk::{arch_program::pubkey::Pubkey, with_secret_key_file, ArchRpcClient, Config};
use autara_client::config::path_from_workspace;

const PROGRAM_KEY: &str = "keys/autara-stage.key";
const AUTHORITY_KEY: &str = "keys/autara-deployer.key";
const EXPECTED_PROGRAM_B58: &str = "6eQ1vLSAwmbT6SD3KQbNawAqis7LpzwpNTd7SJ1GU5cm";

fn b58(pk: &Pubkey) -> String {
    bs58::encode(pk.0).into_string()
}

const BPF_LOADER_B58: &str = "BpfLoader1111111111111111111111111111111111";
const LOADER_HEADER_LEN: usize = 40; // size_of::<bpf_loader::LoaderState>()

fn status_str(s: u64) -> &'static str {
    match s {
        0 => "Retracted (deployable)",
        1 => "Deployed (upgradeable)",
        2 => "Finalized (IMMUTABLE — cannot upgrade)",
        _ => "unknown",
    }
}

fn main() -> anyhow::Result<()> {
    let config = Config {
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network: arch_sdk::arch_program::bitcoin::Network::Testnet4,
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        titan_url: String::new(),
    };

    // These derive the pubkeys from the local keypair files. (with_secret_key_file
    // would *create* a key if the file were missing — so run from the repo root
    // where keys/ exists, or you'll compare against a freshly generated key.)
    let (_pkp, program_id) = with_secret_key_file(&path_from_workspace(PROGRAM_KEY))?;
    let (_akp, expected_authority) = with_secret_key_file(&path_from_workspace(AUTHORITY_KEY))?;

    println!("program keypair: {PROGRAM_KEY}");
    println!("  id (base58): {}", b58(&program_id));
    println!("  id (hex):    {}", hex::encode(program_id.0));
    println!(
        "  matches expected program id: {}",
        b58(&program_id) == EXPECTED_PROGRAM_B58
    );
    println!("expected upgrade authority: {AUTHORITY_KEY}");
    println!("  authority (base58): {}", b58(&expected_authority));
    println!();

    let client = ArchRpcClient::new(&config);
    let acc = client.read_account_info(program_id)?;
    println!("on-chain program account:");
    println!("  owner (loader program): {}", b58(&acc.owner));
    println!(
        "  owner is Arch BPF loader (upgradeable v4): {}",
        b58(&acc.owner) == BPF_LOADER_B58
    );
    println!("  data length: {} bytes", acc.data.len());

    if acc.data.len() >= LOADER_HEADER_LEN {
        // bpf_loader::LoaderState: authority(32) then status(u64 LE), ELF at 40.
        let onchain_authority = Pubkey::from_slice(&acc.data[0..32]);
        let status = u64::from_le_bytes(acc.data[32..40].try_into().unwrap());
        println!();
        println!("loader header (arch_program::bpf_loader::LoaderState):");
        println!("  authority_or_next_version: {}", b58(&onchain_authority));
        println!("  status {status}: {}", status_str(status));
        // Sanity: ELF magic 0x7f should sit right after the 40-byte header.
        println!(
            "  byte[40] = 0x{:02x} (0x7f = ELF magic, confirms 40-byte header)",
            acc.data[LOADER_HEADER_LEN]
        );
        println!();
        println!(
            "PRECONDITION 1 — upgradeable (not finalized): {}",
            status != 2
        );
        println!(
            "PRECONDITION 2 — authority == deployer key:   {}",
            onchain_authority == expected_authority
        );
    } else {
        println!("(account smaller than a loader header; inspect with arch-cli instead.)");
    }
    Ok(())
}
