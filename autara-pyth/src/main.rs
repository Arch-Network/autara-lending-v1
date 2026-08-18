use std::net::SocketAddr;

use arch_program::bitcoin::Network;
use arch_sdk::{generate_new_keypair, with_secret_key_file, AsyncArchRpcClient, Config};
use autara_pyth::{
    fetch_and_push_feeds, push_interval_from_env, start_metrics_server, PusherMetrics,
};
use clap::Parser;

#[derive(clap::Parser, Debug)]
struct Args {
    #[clap(long, default_value = "http://localhost:9002")]
    rpc: String,
    #[clap(long, default_value = "regtest")]
    network: Network,
    #[clap(
        long,
        default_value = "1728b8a70a5502e610b7c77320f3efcbbbab0032c7e66e949d518232045cc185"
    )]
    program_id: String,
    #[clap(
        long,
        value_delimiter = ',',
        default_value = DEFAULT_FEEDS
    )]
    feeds: Vec<String>,
    /// Path to a hex secret-key file for the push signer. When set, the pusher
    /// uses this (pre-funded) key and skips faucet funding — required on
    /// mainnet, which has no faucet. When unset, a throwaway key is generated
    /// and funded via the faucet (testnet/localnet only).
    #[clap(long)]
    signer: Option<String>,
    /// Seconds between push iterations. Falls back to the PUSH_INTERVAL_SECS
    /// env var, then to the default 5s.
    #[clap(long)]
    push_interval_secs: Option<u64>,
    /// Bind address for `/health` and `/metrics`. Defaults to
    /// `0.0.0.0:$PORT` when `PORT` is set (Railway), otherwise disabled.
    #[clap(long)]
    metrics_listen: Option<String>,
}

const DEFAULT_FEEDS:&str = "0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43,0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a";

fn make_config(rpc: &str, network: Network) -> Config {
    Config {
        arch_node_url: rpc.to_string(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    }
}

fn resolve_metrics_listen(arg: Option<String>) -> Option<SocketAddr> {
    if let Some(value) = arg {
        return Some(
            value
                .parse()
                .unwrap_or_else(|e| panic!("invalid --metrics-listen {value}: {e}")),
        );
    }
    std::env::var("PORT").ok().map(|port| {
        format!("0.0.0.0:{port}")
            .parse()
            .unwrap_or_else(|e| panic!("invalid PORT={port}: {e}"))
    })
}

#[tokio::main(flavor = "current_thread")]
pub async fn main() {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::Level::INFO.into())
                .from_env_lossy(),
        )
        .init();
    let config = make_config(&args.rpc, args.network);
    let client = AsyncArchRpcClient::new(&config);
    let authority_keypair = match &args.signer {
        // Pre-funded signer: mainnet has no faucet, so the key must already hold funds.
        Some(path) => {
            with_secret_key_file(path)
                .unwrap_or_else(|e| panic!("failed to load signer key from {path}: {e}"))
                .0
        }
        // No --signer: generate a throwaway key funded by the faucet (testnet/localnet only).
        None => {
            let (keypair, _, _) = generate_new_keypair(args.network);
            client
                .create_and_fund_account_with_faucet(&keypair)
                .await
                .expect("faucet funding failed (mainnet has no faucet — pass --signer)");
            keypair
        }
    };

    let oracle_program_id =
        arch_program::pubkey::Pubkey::from_slice(&hex::decode(&args.program_id).unwrap());
    let push_interval = args
        .push_interval_secs
        .map(std::time::Duration::from_secs)
        .unwrap_or_else(push_interval_from_env);

    let metrics = PusherMetrics::new();
    if let Some(addr) = resolve_metrics_listen(args.metrics_listen) {
        start_metrics_server(addr, metrics.clone())
            .await
            .unwrap_or_else(|e| panic!("failed to bind metrics listen {addr}: {e}"));
    }

    fetch_and_push_feeds(
        &client,
        &oracle_program_id,
        &authority_keypair,
        &args.feeds,
        args.network,
        push_interval,
        Some(metrics),
    )
    .await
}
