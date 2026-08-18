//! Fund an existing Arch key from the testnet/localnet faucet.
//!
//! Usage:
//!   cargo run -p autara-client --example fund_signer -- \
//!     --key autara-deploy/.keys-testnet/pusher.key \
//!     --rpc https://rpc.testnet.arch.network \
//!     --network testnet

use anyhow::{bail, Context, Result};
use arch_sdk::arch_program::bitcoin::Network;
use arch_sdk::{with_secret_key_file, AsyncArchRpcClient, Config};
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    /// Path to a 64-hex secret key file (`with_secret_key_file` format).
    #[clap(long)]
    key: String,
    #[clap(long, default_value = "https://rpc.testnet.arch.network")]
    rpc: String,
    #[clap(long, default_value = "testnet")]
    network: String,
}

fn parse_network(s: &str) -> Result<Network> {
    Ok(match s.to_lowercase().as_str() {
        "mainnet" | "bitcoin" => {
            bail!("refusing to faucet-fund on mainnet (no faucet)")
        }
        "testnet4" => Network::Testnet4,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        other => bail!("unknown network '{other}'"),
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let network = parse_network(&args.network)?;
    let (keypair, pubkey) =
        with_secret_key_file(&args.key).map_err(|e| anyhow::anyhow!("load {}: {e}", args.key))?;

    let config = Config {
        arch_node_url: args.rpc.clone(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    };
    let client = AsyncArchRpcClient::new(&config);
    client
        .create_and_fund_account_with_faucet(&keypair)
        .await
        .context("faucet funding failed")?;

    let balance = client
        .read_account_info(pubkey)
        .await
        .map(|a| a.lamports)
        .unwrap_or(0);
    println!("funded pubkey={}", hex::encode(pubkey.serialize()));
    println!("lamports={balance}");
    Ok(())
}
