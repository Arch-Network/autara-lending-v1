mod config;
mod propamm;
mod router;
mod scanner;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use arch_sdk::ArchRpcClient;
use autara_client::client::blockhash_cache::BlockhashCache;
use autara_client::client::single_thread_client::AutaraReadClientImpl;
use clap::Parser;
use itertools::Itertools;
use orca_whirlpools::{WhirlpoolsConfigInput, set_whirlpools_config_address};

use crate::config::{Args, LiquidatorConfig, TokenFilter, parse_hex_pubkey};
use crate::router::{DISCOVERY_INTERVAL, SwapRouter};
use crate::scanner::scan_liquidatable_positions;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();

    let config_str = std::fs::read_to_string(&args.config).context("failed to read config file")?;
    let config: LiquidatorConfig =
        serde_json::from_str(&config_str).context("failed to parse config file")?;

    let autara_program_id = parse_hex_pubkey(&config.autara_program_id)?;
    let network = config.parse_network()?;
    let dry_run = config.dry_run;
    let (liquidator_keypair, liquidator_pubkey) = config.load_keypair()?;
    tracing::info!(
        ?liquidator_pubkey,
        ?network,
        dry_run,
        "Loaded liquidator keypair"
    );

    let token_filter = TokenFilter::from_config(&config.restrict_tokens)?;
    if token_filter.is_active() {
        tracing::info!(
            "Token filter active: restricting to {} token(s)",
            config.restrict_tokens.len()
        );
    }

    let propamm = config.build_propamm()?;
    if let Some(p) = &propamm {
        tracing::info!(
            program = ?p.program_id,
            backend = %p.backend_url,
            "PropAMM venue enabled — routing CLAMM vs PropAMM by output"
        );
    }

    if let Some(ref wp_config) = config.whirlpools_config {
        let wp_pubkey = parse_hex_pubkey(wp_config)?;
        set_whirlpools_config_address(WhirlpoolsConfigInput::Address(wp_pubkey))
            .map_err(|e| anyhow::anyhow!("failed to set whirlpools config: {}", e))?;
    }

    tracing::info!(rpc_url = %config.rpc_url, "Starting liquidator");

    let sdk_config = arch_sdk::Config {
        arch_node_url: config.rpc_url.clone(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    };
    let arch_client = ArchRpcClient::new(&sdk_config);

    let mut read_client = AutaraReadClientImpl::new(arch_client.clone(), autara_program_id);
    let router = Arc::new(SwapRouter::new(arch_client.clone()));
    let blockhash_cache = BlockhashCache::new(arch_client.clone(), None).await?;

    let poll_interval = Duration::from_secs(config.poll_interval_secs);

    tokio::spawn({
        let router = router.clone();
        async move {
            loop {
                router.maybe_refresh_pools().await;
                tokio::time::sleep(DISCOVERY_INTERVAL).await;
            }
        }
    });

    const TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
    let mut last_token_refresh = Instant::now() - TOKEN_REFRESH_INTERVAL;
    loop {
        match read_client.reload().await {
            Ok(()) => {
                scan_liquidatable_positions(
                    &read_client,
                    &router,
                    propamm.as_ref(),
                    &token_filter,
                    &arch_client,
                    autara_program_id,
                    &liquidator_keypair,
                    liquidator_pubkey,
                    &blockhash_cache,
                    network,
                    dry_run,
                )
                .await;
            }
            Err(e) => {
                tracing::error!("Failed to reload state: {:#}", e);
            }
        }
        if last_token_refresh.elapsed() > TOKEN_REFRESH_INTERVAL {
            last_token_refresh = Instant::now();
            let tokens = token_filter.filter_tokens(read_client.all_tokens());
            tokio::spawn({
                let router = router.clone();
                async move {
                    for [token_a, token_b] in tokens.iter().array_combinations::<2>() {
                        let _ = router.register_pair(*token_a, *token_b).await;
                    }
                }
            });
        }

        tokio::time::sleep(poll_interval).await;
    }
}
