use std::sync::Arc;

use anyhow::{Context, Result, bail};
use arch_sdk::AsyncArchRpcClient;
use autara_liquidator::{
    config::{LiquidatorConfig, parse_hex_pubkey},
    propamm::PropAmmClient,
    router::SwapRouter,
};
use clap::Parser;

#[derive(Parser)]
#[command(name = "liquidator-check-venues")]
#[command(about = "Read-only CLAMM readiness and exact-input RFQ quote check")]
struct Args {
    #[arg(long, default_value = "liquidator-config.json")]
    config: String,
    #[arg(long)]
    user: String,
    #[arg(long)]
    input_mint: String,
    #[arg(long)]
    output_mint: String,
    #[arg(long)]
    amount: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config: LiquidatorConfig = serde_json::from_str(
        &std::fs::read_to_string(&args.config).context("failed to read config")?,
    )
    .context("failed to parse config")?;
    let network = config.parse_network()?;
    let rpc = AsyncArchRpcClient::new(&arch_sdk::Config {
        arch_node_url: config.rpc_url,
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    });
    let user = parse_hex_pubkey(&args.user)?;
    let input_mint = parse_hex_pubkey(&args.input_mint)?;
    let output_mint = parse_hex_pubkey(&args.output_mint)?;
    let user_account = rpc
        .read_account_info(user)
        .await
        .context("failed to read the configured user account")?;
    println!("user_native_balance_lamports: {}", user_account.lamports);
    let mut working_venues = 0usize;

    if let Some(config) = &config.clamm {
        let router = Arc::new(SwapRouter::new(
            rpc.clone(),
            parse_hex_pubkey(&config.program_id)?,
            parse_hex_pubkey(&config.config_pubkey)?,
            config.slippage_bps,
        )?);
        for pool in &config.pools {
            router
                .add_static_pool(
                    parse_hex_pubkey(&pool.token_a)?,
                    parse_hex_pubkey(&pool.token_b)?,
                    parse_hex_pubkey(&pool.pool)?,
                )
                .await;
        }
        match router.check_readiness().await {
            Ok(ready) => {
                println!("clamm_ready_pools: {ready}");
                match router
                    .best_quote_exact_in(input_mint, output_mint, args.amount, user)
                    .await
                {
                    Ok(Some(quote)) => {
                        println!(
                            "clamm_quote: input={} estimated_output={} pool={:?}",
                            quote.amount_in, quote.estimated_out, quote.execution.pool
                        );
                        working_venues += 1;
                    }
                    Ok(None) => println!("clamm_quote: unavailable"),
                    Err(error) => println!("clamm_quote_error: {error:#}"),
                }
            }
            Err(error) => println!("clamm_ready: false error={error:#}"),
        }
    } else {
        println!("clamm: not configured");
    }

    if let Some(config) = &config.propamm {
        let propamm = PropAmmClient::new(config)?;
        match propamm.check_readiness().await {
            Ok(()) => {
                println!("propamm_ready: true");
                match propamm
                    .quote_exact_in(input_mint, output_mint, args.amount, user)
                    .await
                {
                    Ok(Some(quote)) => {
                        println!(
                            "propamm_quote: input={} estimated_output={} side={:?} expiry_ts={}",
                            quote.amount_in,
                            quote.estimated_out,
                            quote.execution.side,
                            quote.execution.expiry_ts
                        );
                        working_venues += 1;
                    }
                    Ok(None) => println!("propamm_quote: unavailable"),
                    Err(error) => println!("propamm_quote_error: {error:#}"),
                }
            }
            Err(error) => println!("propamm_ready: false error={error:#}"),
        }
    } else {
        println!("propamm: not configured");
    }
    if working_venues == 0 {
        bail!("no configured venue produced a positive exact-input quote");
    }
    Ok(())
}
