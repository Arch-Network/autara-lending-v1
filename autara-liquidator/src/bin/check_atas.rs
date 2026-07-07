//! Ops helper: verify the liquidator account is funded and its ATAs exist for
//! every mint in `restrict_tokens`; create missing ATAs with `--create`.
//!
//! Usage:
//!   check-atas [--config liquidator-config.json] [--create]

use anyhow::{Context, Result};
use apl_token::state::Account as TokenAccount;
use arch_sdk::arch_program::program_pack::Pack;
use arch_sdk::arch_program::pubkey::Pubkey;
use arch_sdk::arch_program::sanitized::ArchMessage;
use arch_sdk::{build_and_sign_transaction, ArchRpcClient, Status};
use autara_lib::token::{create_ata_ix, get_associated_token_address};

fn parse_hex_pubkey(hex_str: &str) -> Result<Pubkey> {
    let bytes = hex::decode(hex_str).context("invalid hex for pubkey")?;
    Ok(Pubkey::from_slice(&bytes))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let create = args.iter().any(|a| a == "--create");
    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("liquidator-config.json");

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).context("read config")?)
            .context("parse config")?;
    let rpc_url = config["rpc_url"].as_str().context("rpc_url missing")?;
    let keypair_path = config["liquidator_keypair"]
        .as_str()
        .context("liquidator_keypair missing")?;
    let network = match config["network"].as_str().unwrap_or("regtest") {
        "regtest" => arch_sdk::arch_program::bitcoin::Network::Regtest,
        "testnet" | "testnet3" => arch_sdk::arch_program::bitcoin::Network::Testnet,
        "testnet4" => arch_sdk::arch_program::bitcoin::Network::Testnet4,
        "signet" => arch_sdk::arch_program::bitcoin::Network::Signet,
        "bitcoin" | "mainnet" => arch_sdk::arch_program::bitcoin::Network::Bitcoin,
        other => anyhow::bail!("unknown network: {other}"),
    };
    let mints: Vec<Pubkey> = config["restrict_tokens"]
        .as_array()
        .context("restrict_tokens missing")?
        .iter()
        .map(|v| parse_hex_pubkey(v.as_str().unwrap_or_default()))
        .collect::<Result<_>>()?;

    let (keypair, owner) =
        arch_sdk::with_secret_key_file(keypair_path).context("load liquidator keypair")?;

    let sdk_config = arch_sdk::Config {
        arch_node_url: rpc_url.to_string(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    };
    let client = ArchRpcClient::new(&sdk_config);

    match client.read_account_info(owner).await {
        Ok(info) => println!("liquidator {owner:?}: {} lamports", info.lamports),
        Err(e) => println!("liquidator {owner:?}: account not found ({e}) — FUND IT"),
    }

    let mut missing: Vec<(Pubkey, Pubkey)> = Vec::new();
    for mint in &mints {
        let ata = get_associated_token_address(&owner, mint);
        match client.read_account_info(ata).await {
            Ok(info) => match TokenAccount::unpack(&info.data) {
                Ok(acc) => println!("mint {mint:?}: ATA {ata:?} EXISTS, balance={}", acc.amount),
                Err(e) => println!("mint {mint:?}: ATA {ata:?} exists but unpack failed: {e}"),
            },
            Err(_) => {
                println!("mint {mint:?}: ATA {ata:?} MISSING");
                missing.push((*mint, ata));
            }
        }
    }

    if missing.is_empty() {
        println!("all ATAs present");
        return Ok(());
    }
    if !create {
        println!("run with --create to create the missing ATA(s)");
        return Ok(());
    }

    let ixs: Vec<_> = missing
        .iter()
        .map(|(mint, ata)| create_ata_ix(&owner, Some(ata), &owner, mint))
        .collect();
    let blockhash = client.get_best_block_hash().await?.try_into()?;
    let message = ArchMessage::new(&ixs, Some(owner), blockhash);
    let tx = build_and_sign_transaction(message, vec![keypair], network)?;
    let txids = client.send_transactions(vec![tx]).await?;
    println!("sent create-ATA tx: {:?}", txids);
    let processed = client.wait_for_processed_transactions(txids).await?;
    for p in &processed {
        if p.status == Status::Processed {
            println!("tx processed OK");
        } else {
            println!("tx status={:?} logs={:?}", p.status, p.logs);
        }
    }
    Ok(())
}
