use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use arch_sdk::AsyncArchRpcClient;
use autara_client::client::{
    blockhash_cache::BlockhashCache, single_thread_client::AutaraReadClientImpl,
};
use autara_liquidator::{
    config::{Args, LiquidatorConfig, TokenFilter, parse_hex_pubkey},
    propamm::PropAmmClient,
    router::SwapRouter,
    scanner::scan_liquidatable_positions,
};
use clap::Parser;

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
    let (liquidator_keypair, liquidator_pubkey) = config.load_keypair()?;
    tracing::info!(
        ?liquidator_pubkey,
        ?network,
        dry_run = config.dry_run,
        "Loaded liquidator keypair"
    );
    let token_filter = TokenFilter::from_config(&config.restrict_tokens)?;
    if token_filter.is_active() {
        tracing::info!(
            token_count = config.restrict_tokens.len(),
            "Token filter active"
        );
    }

    let sdk_config = arch_sdk::Config {
        arch_node_url: config.rpc_url.clone(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    };
    let arch_client = AsyncArchRpcClient::new(&sdk_config);
    let lending_program = arch_client
        .read_account_info(autara_program_id)
        .await
        .context("failed to read configured Autara lending program")?;
    if !lending_program.is_executable {
        bail!("configured Autara lending program account is not executable");
    }
    let liquidator_account = arch_client
        .read_account_info(liquidator_pubkey)
        .await
        .context("failed to read liquidator Arch account")?;
    if liquidator_account.lamports == 0 {
        bail!("liquidator Arch account has no native fee balance");
    }
    tracing::info!(
        lamports = liquidator_account.lamports,
        "Liquidator native balance ready"
    );
    let propamm = config
        .propamm
        .as_ref()
        .map(PropAmmClient::new)
        .transpose()?;
    let router = if let Some(clamm) = &config.clamm {
        let router = Arc::new(SwapRouter::new(
            arch_client.clone(),
            parse_hex_pubkey(&clamm.program_id).context("invalid CLAMM program_id")?,
            parse_hex_pubkey(&clamm.config_pubkey).context("invalid CLAMM config_pubkey")?,
            clamm.slippage_bps,
        )?);
        for configured_pool in &clamm.pools {
            router
                .add_static_pool(
                    parse_hex_pubkey(&configured_pool.token_a).context("invalid CLAMM token_a")?,
                    parse_hex_pubkey(&configured_pool.token_b).context("invalid CLAMM token_b")?,
                    parse_hex_pubkey(&configured_pool.pool).context("invalid CLAMM pool")?,
                )
                .await;
        }
        Some(router)
    } else {
        None
    };

    let propamm_ready = match &propamm {
        Some(propamm) => match propamm.check_readiness().await {
            Ok(()) => {
                tracing::info!(
                    base_url = propamm.base_url(),
                    program_id = ?propamm.expected_program_id(),
                    "PropAMM RFQ venue ready"
                );
                true
            }
            Err(error) => {
                tracing::warn!(%error, "PropAMM RFQ venue unavailable at startup");
                false
            }
        },
        None => false,
    };
    let clamm_ready = match &router {
        Some(router) => match router.check_readiness().await {
            Ok(ready_pools) => {
                tracing::info!(ready_pools, "CLAMM venue ready");
                true
            }
            Err(error) => {
                tracing::warn!(%error, "CLAMM venue unavailable at startup");
                false
            }
        },
        None => false,
    };
    if !propamm_ready && !clamm_ready {
        bail!("no configured liquidation venue passed startup readiness");
    }

    tracing::info!(
        rpc_url = %config.rpc_url,
        ?autara_program_id,
        propamm_configured = propamm.is_some(),
        clamm_configured = router.is_some(),
        "Starting liquidator"
    );
    let mut read_client = AutaraReadClientImpl::new(arch_client.clone(), autara_program_id);
    let blockhash_cache = BlockhashCache::new(arch_client.clone(), None).await?;
    let poll_interval = Duration::from_secs(config.poll_interval_secs);

    loop {
        match read_client.reload().await {
            Ok(()) => {
                scan_liquidatable_positions(
                    &read_client,
                    router.as_deref(),
                    propamm.as_ref(),
                    &token_filter,
                    &arch_client,
                    autara_program_id,
                    &liquidator_keypair,
                    liquidator_pubkey,
                    &blockhash_cache,
                    network,
                    config.dry_run,
                )
                .await;
            }
            Err(error) => tracing::error!(%error, "Failed to reload lending state"),
        }
        tokio::time::sleep(poll_interval).await;
    }
}
