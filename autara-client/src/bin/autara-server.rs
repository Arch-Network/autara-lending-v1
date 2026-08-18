use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{anyhow, Context};
use arch_sdk::arch_program::bitcoin::Network;
use arch_sdk::arch_program::pubkey::Pubkey;
use arch_sdk::with_secret_key_file;
use autara_client::api::server::build_autara_server;
use autara_client::client::client_with_signer::AutaraFullClientWithSigner;
use autara_client::client::read::AutaraReadClient;
use autara_client::client::shared_autara_state::AutaraSharedState;
use autara_client::config::{autara_oracle_stage_program_id, autara_stage_program_id, ArchConfig};
use autara_client::prometheus::autara_indexer::PrometheusAutaraIndexer;
use autara_client::prometheus::exporter::PrometheusExporter;
use autara_client::test::TokenMinter;
use autara_lib::interest_rate::interest_rate_kind::InterestRateCurveKind;
use autara_lib::ixs::CreateMarketInstruction;
use autara_lib::oracle::oracle_config::OracleConfig;
use autara_lib::pda::find_market_pda;
use autara_lib::state::market_config::LtvConfig;
use autara_pyth::{fetch_and_push_feeds, BTC_FEED, ETH_FEED, USDC_FEED};
use clap::Parser;
use jsonrpsee::server::{RpcServiceBuilder, Server};
use serde::Deserialize;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "autara-server")]
#[command(about = "Autara Lending API server")]
struct Args {
    /// Path to tokens.json config file (produced by `autara-cli token setup`)
    #[arg(long)]
    tokens: String,

    /// Autara program ID (hex pubkey). Defaults to keys/autara-stage.key address.
    #[arg(long)]
    program_id: Option<String>,

    /// Oracle program ID (hex pubkey). Defaults to keys/autara-pyth-stage.key address.
    #[arg(long)]
    oracle_program_id: Option<String>,

    /// Path to signer key file (authority for market creation, minting, etc.).
    /// Falls back to AUTARA_SIGNER_KEY env var if not provided.
    #[arg(long)]
    signer: Option<String>,

    /// Arch node URL
    #[arg(long, default_value = "https://rpc.testnet.arch.network")]
    arch_node: String,

    /// Bitcoin network (regtest, testnet, mainnet)
    #[arg(long, default_value = "testnet")]
    network: String,

    /// RPC server listen address
    #[arg(long, default_value = "0.0.0.0:62776")]
    listen: String,

    /// Prometheus exporter listen address
    #[arg(long, default_value = "0.0.0.0:62777")]
    prometheus: String,

    /// Path to market config JSON file (CreateMarketInstruction fields).
    /// If not provided, uses a sensible default config.
    #[arg(long)]
    market_config: Option<String>,
}

#[derive(Deserialize)]
struct TokensFile {
    #[serde(rename = "authorityKeyFile")]
    authority_key_file: String,
    #[allow(dead_code)]
    authority: String,
    tokens: HashMap<String, TokenEntry>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct TokenEntry {
    mint: String,
    decimals: u8,
    #[serde(rename = "keyFile")]
    key_file: String,
    /// Optional per-token faucet amount (raw units) minted to a new user by the
    /// server's `initialize`. Falls back to FAUCET_MINT_AMOUNT (env) / the
    /// DEFAULT_FAUCET_MINT_AMOUNT constant when unset.
    #[serde(rename = "faucetAmount")]
    faucet_amount: Option<u64>,
}

/// Default per-user faucet amount (raw units) when neither tokens.json nor the
/// FAUCET_MINT_AMOUNT env var specifies one. Mirrors CLAMM's TEST_MINT_AMOUNT
/// (1e12 raw); replaces the previously hardcoded 100_000_000_000.
const DEFAULT_FAUCET_MINT_AMOUNT: u64 = 1_000_000_000_000;

fn parse_network(network: &str) -> anyhow::Result<Network> {
    match network.to_lowercase().as_str() {
        "regtest" => Ok(Network::Regtest),
        "testnet" => Ok(Network::Testnet),
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        _ => anyhow::bail!("Invalid network: {network}. Use regtest, testnet, or mainnet"),
    }
}

fn parse_pubkey(s: &str) -> anyhow::Result<Pubkey> {
    if s.len() == 64 {
        let bytes: [u8; 32] = hex::decode(s)
            .context("Invalid hex pubkey")?
            .try_into()
            .map_err(|_| anyhow!("Invalid pubkey length"))?;
        return Ok(Pubkey::from(bytes));
    }
    std::str::FromStr::from_str(s).map_err(|e| anyhow!("Invalid pubkey format: {}", e))
}

/// Map token name -> Pyth feed ID (bytes)
fn pyth_feed_for_token(name: &str) -> Option<[u8; 32]> {
    let feed_hex = match name.to_uppercase().as_str() {
        "BTC" | "ABTC" => BTC_FEED,
        "USDC" | "AUSD" => USDC_FEED,
        "ETH" => ETH_FEED,
        _ => return None,
    };
    let hex_str = feed_hex.strip_prefix("0x").unwrap_or(feed_hex);
    let mut id = [0u8; 32];
    hex::decode_to_slice(hex_str, &mut id).ok()?;
    Some(id)
}

fn default_market_config(
    supply_feed_id: [u8; 32],
    collateral_feed_id: [u8; 32],
    oracle_program_id: Pubkey,
) -> CreateMarketInstruction {
    CreateMarketInstruction {
        market_bump: 0,
        index: 0,
        ltv_config: LtvConfig {
            max_ltv: 0.8.into(),
            unhealthy_ltv: 0.9.into(),
            liquidation_bonus: 0.05.into(),
        },
        max_utilisation_rate: 0.9.into(),
        supply_oracle_config: OracleConfig::new_pyth(supply_feed_id, oracle_program_id),
        collateral_oracle_config: OracleConfig::new_pyth(collateral_feed_id, oracle_program_id),
        interest_rate: InterestRateCurveKind::new_adaptive(),
        lending_market_fee_in_bps: 100,
    }
}

async fn account_exists(client: &arch_sdk::ArchRpcClient, pubkey: &Pubkey) -> bool {
    client.read_account_info(*pubkey).await.is_ok()
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    dotenvy::dotenv().ok();
    let filter = EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(filter)
        .finish()
        .init();

    let args = Args::parse();
    let network = parse_network(&args.network)?;

    let autara_program_id = match args.program_id {
        Some(ref id) => parse_pubkey(id)?,
        None => autara_stage_program_id(),
    };
    let oracle_program_id = match args.oracle_program_id {
        Some(ref id) => parse_pubkey(id)?,
        None => autara_oracle_stage_program_id(),
    };

    // Load signer
    let signer_path = args
        .signer
        .or_else(|| std::env::var("AUTARA_SIGNER_KEY").ok())
        .context("Signer key required: use --signer or set AUTARA_SIGNER_KEY")?;
    let (signer_keypair, signer_pubkey) = with_secret_key_file(&signer_path)
        .context(format!("Failed to load signer key: {}", signer_path))?;

    tracing::info!("Program ID:        {:?}", autara_program_id);
    tracing::info!("Oracle Program ID: {:?}", oracle_program_id);
    tracing::info!("Signer:            {:?}", signer_pubkey);

    // Load tokens config
    let tokens_json = std::fs::read_to_string(&args.tokens)
        .context(format!("Failed to read tokens config: {}", args.tokens))?;
    let tokens_file: TokensFile =
        serde_json::from_str(&tokens_json).context("Failed to parse tokens config")?;
    let (token_authority_keypair, token_authority_pubkey) =
        with_secret_key_file(&tokens_file.authority_key_file).context(format!(
            "Failed to load token authority key: {}",
            tokens_file.authority_key_file
        ))?;
    let tokens = &tokens_file.tokens;
    tracing::info!("Loaded {} tokens from config", tokens.len());
    tracing::info!("Token authority:    {:?}", token_authority_pubkey);

    // Load optional market config override
    let market_config_override: Option<CreateMarketInstruction> =
        if let Some(ref path) = args.market_config {
            let json = std::fs::read_to_string(path)
                .context(format!("Failed to read market config: {}", path))?;
            Some(serde_json::from_str(&json).context("Failed to parse market config JSON")?)
        } else {
            None
        };

    // Setup Arch client
    let config = ArchConfig {
        arch_node_url: args.arch_node.clone(),
        bitcoin_node_endpoint: String::new(),
        bitcoin_node_password: String::new(),
        bitcoin_node_username: String::new(),
    };
    let arch_client = config.arch_rpc_client();

    // Fund signer account via faucet (idempotent, needed for tx fees)
    tracing::info!("Funding signer account via faucet...");
    if let Err(e) = arch_client
        .create_and_fund_account_with_faucet(&signer_keypair)
        .await
    {
        tracing::warn!("Faucet funding failed (may already be funded): {:?}", e);
    }
    tracing::info!("Funding token signer account via faucet...");
    if let Err(e) = arch_client
        .create_and_fund_account_with_faucet(&token_authority_keypair)
        .await
    {
        tracing::warn!("Faucet funding failed (may already be funded): {:?}", e);
    }

    // Create the client for market creation
    let mut client = AutaraFullClientWithSigner::new_simple(
        arch_client.clone(),
        network,
        autara_program_id,
        signer_keypair,
    );

    // Load state
    tracing::info!("Loading protocol state...");
    tokio::time::timeout(Duration::from_secs(60), client.full_reload())
        .await
        .context("Timeout loading protocol state")??;
    tracing::info!("Protocol state loaded");

    // Create global config if not exists
    let global_config = client.read_client().get_global_config();
    if global_config.is_none() {
        tracing::info!("Creating global config...");
        client
            .create_global_config(signer_pubkey, signer_pubkey, 100)
            .await
            .context("Failed to create global config")?;
        tracing::info!("Global config created");
    } else {
        tracing::info!("Global config already exists");
    }

    let token_names: Vec<String> = tokens.keys().cloned().collect();
    // Create markets for all token pair combinations (if not already existing)
    let token_list: Vec<(String, &TokenEntry)> =
        tokens.iter().map(|(k, v)| (k.clone(), v)).collect();
    for i in 0..token_list.len() {
        for j in 0..token_list.len() {
            if i == j {
                continue;
            }
            let (supply_name, supply_token) = &token_list[i];
            let (collateral_name, collateral_token) = &token_list[j];

            let supply_mint = parse_pubkey(&supply_token.mint)?;
            let collateral_mint = parse_pubkey(&collateral_token.mint)?;

            let supply_feed_id = pyth_feed_for_token(supply_name);
            let collateral_feed_id = pyth_feed_for_token(collateral_name);

            if supply_feed_id.is_none() || collateral_feed_id.is_none() {
                tracing::warn!(
                    "Skipping market {}/{}: no Pyth feed mapping",
                    supply_name,
                    collateral_name
                );
                continue;
            }

            let (market_pda, _) = find_market_pda(
                &autara_program_id,
                &signer_pubkey,
                &supply_mint,
                &collateral_mint,
                0,
            );

            if account_exists(&arch_client, &market_pda).await {
                tracing::info!(
                    "Market {}/{} already exists at {:?}",
                    supply_name,
                    collateral_name,
                    market_pda
                );
                continue;
            }

            let market_ix = if let Some(ref config_override) = market_config_override {
                let mut ix = config_override.clone();
                ix.supply_oracle_config =
                    OracleConfig::new_pyth(supply_feed_id.unwrap(), oracle_program_id);
                ix.collateral_oracle_config =
                    OracleConfig::new_pyth(collateral_feed_id.unwrap(), oracle_program_id);
                ix
            } else {
                default_market_config(
                    supply_feed_id.unwrap(),
                    collateral_feed_id.unwrap(),
                    oracle_program_id,
                )
            };

            tracing::info!("Creating market {}/{}...", supply_name, collateral_name);
            match client
                .create_market(market_ix, supply_mint, collateral_mint)
                .await
            {
                Ok(market_pubkey) => {
                    tracing::info!(
                        "Market {}/{} created at {:?}",
                        supply_name,
                        collateral_name,
                        market_pubkey
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to create market {}/{}: {:?}",
                        supply_name,
                        collateral_name,
                        e
                    );
                }
            }
        }
    }

    // Spawn Pyth feed pusher in background
    let feeds: Vec<String> = token_names
        .iter()
        .filter_map(|name| {
            let feed_hex = match name.to_uppercase().as_str() {
                "BTC" | "ABTC" => Some(BTC_FEED),
                "USDC" | "AUSD" => Some(USDC_FEED),
                "ETH" => Some(ETH_FEED),
                _ => None,
            }?;
            Some(feed_hex.to_string())
        })
        .collect();
    // When a dedicated `autara-pyth` service pushes prices (the mainnet/testnet
    // symmetric setup), set DISABLE_PRICE_PUSHER=1 here so the server doesn't
    // also push the same feeds. Default (unset) keeps the built-in pusher.
    let disable_price_pusher = std::env::var("DISABLE_PRICE_PUSHER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !feeds.is_empty() && !disable_price_pusher {
        tracing::info!("Spawning Pyth feed pusher for {} feeds", feeds.len());
        let pusher_client = arch_client.clone();
        let pusher_signer = signer_keypair;
        let to_airdrop = signer_pubkey;
        tokio::spawn(async move {
            fetch_and_push_feeds(
                &pusher_client,
                &oracle_program_id,
                &pusher_signer,
                &feeds,
                network,
                autara_pyth::push_interval_from_env(),
                None,
            )
            .await;
        });
        let aidrop_client = arch_client.clone();
        tokio::spawn(async move {
            loop {
                let _ = aidrop_client.request_airdrop(to_airdrop).await;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    // Per-user faucet amount default: FAUCET_MINT_AMOUNT env, else the constant.
    let default_faucet_amount: u64 = match std::env::var("FAUCET_MINT_AMOUNT")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(v) => v
            .trim()
            .parse()
            .context("invalid FAUCET_MINT_AMOUNT (expected raw u64)")?,
        None => DEFAULT_FAUCET_MINT_AMOUNT,
    };

    // Build token minters from config (using token authority key), each paired
    // with its per-token faucet amount.
    let mut minters = Vec::new();
    for (name, token) in tokens {
        let mint = parse_pubkey(&token.mint)?;
        let faucet_amount = token.faucet_amount.unwrap_or(default_faucet_amount);
        let minter = TokenMinter::from_existing(arch_client.clone(), token_authority_keypair, mint);
        tracing::info!(
            "Token minter for {} ({:?}) faucet_amount={}",
            name,
            mint,
            faucet_amount
        );
        minters.push((minter, faucet_amount));
    }

    // Start the shared state (auto-reloading)
    let autara_state = AutaraSharedState::new(arch_client.clone(), autara_program_id)
        .spawn()
        .0;

    // Build and start the RPC server
    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_origin(Any);
    let server = Server::builder()
        .max_response_body_size(100 * 1024 * 1024) // 100 MB (before compression)
        .set_http_middleware(
            tower::ServiceBuilder::new()
                .timeout(Duration::from_secs(60))
                .layer(cors)
                .layer(CompressionLayer::new()),
        )
        .set_rpc_middleware(
            RpcServiceBuilder::new().layer(autara_client::api::tracing::AutaraTraceLayer),
        )
        .build(args.listen.parse::<SocketAddr>()?)
        .await?;
    let module = build_autara_server(autara_state.clone(), minters, arch_client).await?;
    let server_addr = server.local_addr()?;
    let handle = server.start(module);
    tracing::info!("Server running at: {}", server_addr);

    // Start Prometheus
    PrometheusExporter::launch(args.prometheus.parse()?, None).await?;
    tracing::info!("Prometheus exporter running at {}", args.prometheus);
    PrometheusAutaraIndexer::new(autara_state, Duration::from_secs(60)).spawn();

    handle.stopped().await;
    Ok(())
}
