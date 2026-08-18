//! Read-only: dump the CURRENT on-chain program ELF to a file, so you have a
//! known-good rollback target before running an upgrade.
//!
//! Run from repo root (before upgrading):
//!   cargo run -p autara-client --bin backup_program_elf
//!   # optional custom path: ... --bin backup_program_elf -- backups/old.so
//!
//! To roll back later: copy the backup over target/deploy/autara_program.so and
//! re-run `upgrade_program` (it will write the old bytes back at the same id).

use std::{fs, path::Path};

use arch_sdk::{arch_program::bpf_loader::LoaderState, ArchRpcClient, Config};

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
    let program_id = autara_program::id();
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "backups/autara_program_onchain.so".to_string());

    let client = ArchRpcClient::new(&config);
    let acc = client
        .read_account_info(program_id)
        .map_err(|e| anyhow::anyhow!("read program account: {e}"))?;

    let offset = LoaderState::program_data_offset();
    anyhow::ensure!(
        acc.data.len() > offset,
        "program account has no ELF payload (len {})",
        acc.data.len()
    );
    let elf = &acc.data[offset..];

    // Never overwrite a good backup with a half-written one. A non-executable
    // program means an upgrade is in flight (retracted, ELF partially rewritten),
    // and the operator is told to re-run the upgrade script — which calls this
    // first. Clobbering here would destroy the only rollback target for a live,
    // fund-holding program. Skip rather than fail, so the re-run can proceed.
    if !acc.is_executable && Path::new(&out).exists() {
        println!(
            "program is not executable (upgrade in progress) — preserving existing backup at {out}"
        );
        return Ok(());
    }

    if let Some(parent) = Path::new(&out).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&out, elf)?;
    println!(
        "backed up {} bytes of on-chain ELF for {} -> {}",
        elf.len(),
        bs58::encode(program_id.0).into_string(),
        out
    );
    Ok(())
}
