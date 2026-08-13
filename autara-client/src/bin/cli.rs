use std::{collections::HashMap, str::FromStr, time::Duration};

use anyhow::{anyhow, Context, Result};
use arch_sdk::{
    arch_program::{
        bitcoin::{key::Keypair, Network},
        program_pack::Pack,
        pubkey::Pubkey,
        rent::minimum_rent,
        sanitized::ArchMessage,
    },
    build_and_sign_transaction, generate_new_keypair, with_secret_key_file, Status,
};
use autara_client::{
    client::{client_with_signer::AutaraFullClientWithSigner, read::AutaraReadClient},
    config::{autara_oracle_stage_program_id, autara_stage_program_id, ArchConfig},
    rpc_ext::ArchAsyncRpcExt,
};
use autara_lib::{
    ixs::CreateMarketInstruction,
    oracle::pyth::PythPrice,
    token::{create_ata_ix, get_associated_token_address},
};
use autara_pyth::{
    fetch_and_push_feeds, fetch_pyth_price, get_pyth_account, push_interval_from_env,
    AutaraPythPusherClient,
};
use clap::{Parser, Subcommand};
use tracing_subscriber::{util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "autara-cli")]
#[command(about = "CLI to interact with the Autara Lending protocol", long_about = None)]
struct Cli {
    /// Arch node URL
    #[arg(long, default_value = "https://rpc.testnet.arch.network")]
    arch_node: String,

    /// Path to the signer key file. Can also be set via AUTARA_SIGNER_KEY env var.
    #[arg(long)]
    signer: Option<String>,

    /// Network to use (regtest, testnet, mainnet)
    #[arg(long, default_value = "regtest")]
    network: String,

    /// Path to tokens.json config (for resolving token names in output)
    #[arg(long, default_value = "tokens.json")]
    tokens: String,

    /// Autara program id (hex or base58). Defaults to the stage program id.
    #[arg(long)]
    program_id: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Read operations
    #[command(subcommand)]
    Read(ReadCommands),

    /// Transaction operations
    #[command(subcommand)]
    Tx(TxCommands),

    /// Token operations (create, mint, list accounts)
    #[command(subcommand)]
    Token(TokenCommands),

    /// Oracle operations (fetch, push, inspect feeds)
    #[command(subcommand)]
    Oracle(OracleCommands),
}

#[derive(Subcommand)]
enum ReadCommands {
    /// List all markets
    Markets,

    /// Get market details
    Market {
        /// Market pubkey
        #[arg(long)]
        market: String,
    },

    /// Get user positions for a specific authority
    Positions {
        /// Authority pubkey (defaults to signer)
        #[arg(long)]
        authority: Option<String>,
    },

    /// Get supply position for a market
    SupplyPosition {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Authority pubkey (defaults to signer)
        #[arg(long)]
        authority: Option<String>,
    },

    /// Get borrow position health for a market
    BorrowHealth {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Authority pubkey (defaults to signer)
        #[arg(long)]
        authority: Option<String>,
    },

    /// Get global config
    GlobalConfig,
}

#[derive(Subcommand)]
enum TxCommands {
    /// Create a new lending market from a JSON config file
    CreateMarket {
        /// Path to JSON config file containing CreateMarketInstruction fields
        #[arg(long)]
        config: String,

        /// Supply token mint pubkey
        #[arg(long)]
        supply_mint: String,

        /// Collateral token mint pubkey
        #[arg(long)]
        collateral_mint: String,
    },

    /// Supply assets to a market
    Supply {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms (base units)
        #[arg(long)]
        amount: u64,
    },

    /// Withdraw supply from a market
    WithdrawSupply {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms (None = withdraw all)
        #[arg(long)]
        amount: Option<u64>,
    },

    /// Deposit collateral to a market
    DepositCollateral {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms
        #[arg(long)]
        amount: u64,
    },

    /// Withdraw collateral from a market
    WithdrawCollateral {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms (None = withdraw all)
        #[arg(long)]
        amount: Option<u64>,
    },

    /// Borrow assets from a market
    Borrow {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms
        #[arg(long)]
        amount: u64,
    },

    /// Repay borrowed assets
    Repay {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms (None = repay all)
        #[arg(long)]
        amount: Option<u64>,
    },

    /// Liquidate an unhealthy position
    Liquidate {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Borrow position pubkey to liquidate
        #[arg(long)]
        position: String,

        /// Max borrowed atoms to repay
        #[arg(long)]
        max_repay: Option<u64>,

        /// Min collateral atoms to receive
        #[arg(long)]
        min_collateral: Option<u64>,
    },

    /// Redeem curator fees
    RedeemCuratorFees {
        /// Market pubkey
        #[arg(long)]
        market: String,
    },

    /// Redeem protocol fees
    RedeemProtocolFees {
        /// Market pubkey
        #[arg(long)]
        market: String,
    },

    /// Donate supply to a market
    DonateSupply {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Amount in atoms
        #[arg(long)]
        amount: u64,
    },

    /// Socialize the loss of an underwater borrow position (curator only, requires LTV >= 1)
    SocializeLoss {
        /// Market pubkey
        #[arg(long)]
        market: String,

        /// Authority of the borrow position to socialize
        #[arg(long)]
        authority: String,
    },
}

#[derive(Subcommand)]
enum TokenCommands {
    /// Create a new token mint
    CreateToken {
        /// Number of decimals for the token (default: 9)
        #[arg(long)]
        decimals: u8,
    },

    /// Mint tokens to an address
    Mint {
        /// Token mint pubkey
        #[arg(long)]
        token: String,

        /// Recipient pubkey (defaults to signer)
        #[arg(long)]
        to: Option<String>,

        /// Amount in base units (atoms)
        #[arg(long)]
        amount: u64,
    },

    /// List all token accounts for an owner
    ListAccounts {
        /// Owner pubkey (defaults to signer)
        #[arg(long)]
        owner: Option<String>,
    },

    /// Create BTC, USDC, ETH token mints (if not already on-chain) and write tokens.json config
    Setup {
        /// Output path for the tokens config file
        #[arg(long, default_value = "tokens.json")]
        output: String,
    },
}

#[derive(Subcommand)]
enum OracleCommands {
    /// Fetch latest price from Pyth API for given feed IDs
    FetchPrice {
        /// Pyth feed IDs (hex, with or without 0x prefix)
        #[arg(long, num_args = 1..)]
        feed: Vec<String>,
    },

    /// Push a dummy price to the on-chain oracle (for testing).
    /// The signer that first creates a feed becomes its bound authority;
    /// later pushes to that feed must use the same signer.
    PushPrice {
        /// Pyth feed ID (hex, with or without 0x prefix)
        #[arg(long)]
        feed: String,

        /// Price value (e.g. 100000.0 for BTC)
        #[arg(long)]
        price: f64,
    },

    /// Continuously fetch and push Pyth feeds to on-chain oracle.
    /// The signer that first creates a feed becomes its bound authority;
    /// later pushes to that feed must use the same signer.
    PushFeeds {
        /// Pyth feed IDs (hex, with 0x prefix)
        #[arg(long, num_args = 1..)]
        feed: Vec<String>,

        /// Seconds between push iterations. Falls back to the
        /// PUSH_INTERVAL_SECS env var, then to the default 5s.
        #[arg(long)]
        push_interval_secs: Option<u64>,
    },

    /// Show oracle feed info for a market
    MarketFeeds {
        /// Market pubkey
        #[arg(long)]
        market: String,
    },
}

fn parse_network(network: &str) -> Result<Network> {
    match network.to_lowercase().as_str() {
        "regtest" => Ok(Network::Regtest),
        "testnet" => Ok(Network::Testnet),
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        _ => anyhow::bail!(
            "Invalid network: {}. Use regtest, testnet, or mainnet",
            network
        ),
    }
}

fn parse_pubkey(s: &str) -> Result<Pubkey> {
    // Try hex decode first (64 chars = 32 bytes)
    if s.len() == 64 {
        let bytes: [u8; 32] = hex::decode(s)
            .context("Invalid hex pubkey")?
            .try_into()
            .map_err(|_| anyhow!("Invalid pubkey length"))?;
        return Ok(Pubkey::from(bytes));
    }
    // Try base58
    Pubkey::from_str(s).map_err(|e| anyhow!("Invalid pubkey format: {}", e))
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(filter)
        .finish()
        .init();

    let cli = Cli::parse();

    let signer_path = cli
        .signer
        .or_else(|| std::env::var("AUTARA_SIGNER_KEY").ok());

    // Setup config
    let config = ArchConfig {
        arch_node_url: cli.arch_node.clone(),
        bitcoin_node_endpoint: String::new(),
        bitcoin_node_password: String::new(),
        bitcoin_node_username: String::new(),
    };

    let network = parse_network(&cli.network)?;
    let arch_client = config.arch_rpc_client();

    // Load signer keypair
    let signer = if let Some(ref path) = signer_path {
        with_secret_key_file(path)
            .context("Failed to load signer key")?
            .0
    } else {
        // Use default stage admin key for development
        autara_client::config::autara_stage_admin()
    };

    let signer_pubkey = Pubkey::from_slice(&signer.x_only_public_key().0.serialize());
    tracing::info!("Using signer: {:?}", signer_pubkey);

    // Resolve program id (default: stage program id)
    let program_id = match cli.program_id {
        Some(ref s) => parse_pubkey(s)?,
        None => autara_stage_program_id(),
    };

    // Create client
    let mut client =
        AutaraFullClientWithSigner::new_simple(arch_client, network, program_id, signer);

    // Load state
    tracing::info!("Loading protocol state...");
    tokio::time::timeout(Duration::from_secs(60), client.full_reload())
        .await
        .context("could not load autara state with RPC")??;
    tracing::info!("Loaded protocol state...");

    let token_names = TokenNames::load(&cli.tokens);

    match cli.command {
        Commands::Read(read_cmd) => handle_read_command(&client, read_cmd, &token_names).await,
        Commands::Tx(tx_cmd) => handle_tx_command(&client, tx_cmd, &token_names).await,
        Commands::Token(token_cmd) => {
            handle_token_command(&client, token_cmd, network, signer, &token_names).await
        }
        Commands::Oracle(oracle_cmd) => {
            handle_oracle_command(&client, oracle_cmd, network, signer).await
        }
    }
}

async fn handle_read_command(
    client: &AutaraFullClientWithSigner<
        autara_client::client::single_thread_client::AutaraReadClientImpl,
    >,
    cmd: ReadCommands,
    tn: &TokenNames,
) -> Result<()> {
    match cmd {
        ReadCommands::Markets => {
            println!("=== Markets ===");
            let mut count = 0;
            for (pubkey, market) in client.read_client().all_markets() {
                count += 1;
                let m = market.market();
                println!("\nMarket: {:?}", pubkey);
                println!("  Supply Token: {}", tn.name(&m.supply_token_info().mint));
                println!("  Supply Decimals: {}", m.supply_token_info().decimals);
                println!(
                    "  Collateral Token: {}",
                    tn.name(&m.collateral_token_info().mint)
                );
                println!(
                    "  Collateral Decimals: {}",
                    m.collateral_token_info().decimals
                );
                println!("  Max LTV: {}", m.config().ltv_config().max_ltv);
                println!("  Unhealthy LTV: {}", m.config().ltv_config().unhealthy_ltv);
                println!(
                    "  Liquidation Bonus: {}",
                    m.config().ltv_config().liquidation_bonus
                );
                println!(
                    "  Max Utilisation Rate: {}",
                    m.config().max_utilisation_rate()
                );
            }
            if count == 0 {
                println!("No markets found");
            } else {
                println!("\nTotal markets: {}", count);
            }
        }

        ReadCommands::Market { market } => {
            let market_key = parse_pubkey(&market)?;
            let market_wrapper = client.read_client().get_market(&market_key);
            if let Some(mw) = market_wrapper {
                let m = mw.market();
                println!("=== Market Details ===");
                println!("Pubkey: {:?}", market_key);
                println!("\nSupply Vault:");
                println!("  Token: {}", tn.name(&m.supply_token_info().mint));
                println!("  Decimals: {}", m.supply_token_info().decimals);
                println!("\nCollateral Vault:");
                println!("  Token: {}", tn.name(&m.collateral_token_info().mint));
                println!("  Decimals: {}", m.collateral_token_info().decimals);
                println!("\nRisk Config:");
                println!("  Max LTV: {}", m.config().ltv_config().max_ltv);
                println!("  Unhealthy LTV: {}", m.config().ltv_config().unhealthy_ltv);
                println!(
                    "  Liquidation Bonus: {}",
                    m.config().ltv_config().liquidation_bonus
                );
                println!("  Max Utilisation: {}", m.config().max_utilisation_rate());
                println!("  Curator: {:?}", m.config().curator());
            } else {
                println!("Market not found: {:?}", market_key);
            }
        }

        ReadCommands::Positions { authority } => {
            let auth = if let Some(ref auth_str) = authority {
                parse_pubkey(auth_str)?
            } else {
                *client.signer_pubkey()
            };
            let positions = client.read_client().user_positions(&auth);

            println!("=== User Positions for {:?} ===", auth);

            println!("\n--- Supply Positions ---");
            if positions.supply_positions.is_empty() {
                println!("  No supply positions");
            } else {
                for pos in &positions.supply_positions {
                    println!("  Market: {:?}", pos.supply_position.market());
                    println!("    Shares: {}", pos.supply_position.shares());
                    println!("    Owned Atoms: {}", pos.owned_atoms);
                    println!();
                }
            }

            println!("--- Borrow Positions ---");
            if positions.borrow_positions.is_empty() {
                println!("  No borrow positions");
            } else {
                for pos in &positions.borrow_positions {
                    println!("  Market: {:?}", pos.borrow_position.market());
                    println!(
                        "    Collateral: {} atoms",
                        pos.borrow_position.collateral_deposited_atoms()
                    );
                    println!(
                        "    Borrowed Shares: {}",
                        pos.borrow_position.borrowed_shares()
                    );
                    println!("    LTV: {}", pos.health.ltv);
                    println!("    Borrow Value: {}", pos.health.borrow_value);
                    println!("    Collateral Value: {}", pos.health.collateral_value);
                    println!();
                }
            }
        }

        ReadCommands::SupplyPosition { market, authority } => {
            let market_key = parse_pubkey(&market)?;
            let auth = if let Some(ref auth_str) = authority {
                parse_pubkey(auth_str)?
            } else {
                *client.signer_pubkey()
            };

            let (pda, position) = client.read_client().get_supply_position(&market_key, &auth);
            println!("=== Supply Position ===");
            println!("PDA: {:?}", pda);
            if let Some(pos) = position {
                println!("Market: {:?}", pos.market());
                println!("Authority: {:?}", pos.authority());
                println!("Shares: {}", pos.shares());

                if let Some(market_wrapper) = client.read_client().get_market(&market_key) {
                    if let Ok(atoms) = market_wrapper.market().supply_position_info(&pos) {
                        println!("Owned Atoms: {}", atoms);
                    }
                }
            } else {
                println!("No supply position found");
            }
        }

        ReadCommands::BorrowHealth { market, authority } => {
            let market_key = parse_pubkey(&market)?;
            let auth = if let Some(ref auth_str) = authority {
                parse_pubkey(auth_str)?
            } else {
                *client.signer_pubkey()
            };

            match client
                .read_client()
                .get_borrow_position_health(&market_key, &auth)
            {
                Ok(health) => {
                    println!("=== Borrow Position Health ===");
                    println!("Market: {:?}", market_key);
                    println!("Authority: {:?}", auth);
                    println!("LTV: {}", health.ltv);
                    println!("Borrowed Atoms: {}", health.borrowed_atoms);
                    println!("Collateral Atoms: {}", health.collateral_atoms);
                    println!("Borrow Value: {}", health.borrow_value);
                    println!("Collateral Value: {}", health.collateral_value);
                }
                Err(e) => {
                    println!("Could not get borrow position health: {}", e);
                }
            }
        }

        ReadCommands::GlobalConfig => {
            if let Some(config) = client.read_client().get_global_config() {
                println!("=== Global Config ===");
                println!("Admin: {:?}", config.admin());
                println!("Fee Receiver: {:?}", config.fee_receiver());
                println!(
                    "Protocol Fee Share: {} bps",
                    config.protocol_fee_share_in_bps()
                );
            } else {
                println!("Global config not found");
            }
        }
    }
    Ok(())
}

async fn handle_tx_command(
    client: &AutaraFullClientWithSigner<
        autara_client::client::single_thread_client::AutaraReadClientImpl,
    >,
    cmd: TxCommands,
    tn: &TokenNames,
) -> Result<()> {
    match cmd {
        TxCommands::CreateMarket {
            config,
            supply_mint,
            collateral_mint,
        } => {
            let supply_mint_key = parse_pubkey(&supply_mint)?;
            let collateral_mint_key = parse_pubkey(&collateral_mint)?;

            let config_json = std::fs::read_to_string(&config)
                .context(format!("Failed to read config file: {}", config))?;
            let create_market_ix: CreateMarketInstruction =
                serde_json::from_str(&config_json).context("Failed to parse market config JSON")?;

            println!(
                "Creating market with supply mint {} and collateral mint {}...",
                tn.name(&supply_mint_key),
                tn.name(&collateral_mint_key)
            );
            let market_pubkey = client
                .create_market(create_market_ix, supply_mint_key, collateral_mint_key)
                .await?;
            println!("Market created successfully!");
            println!("Market: {:?}", market_pubkey);
        }

        TxCommands::Supply { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!("Supplying {} atoms to market {:?}...", amount, market_key);
            let events = client.supply(&market_key, amount).await?;
            println!("Supply successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::WithdrawSupply { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!(
                "Withdrawing {:?} atoms from market {:?}...",
                amount, market_key
            );
            let events = client.withdraw_supply(&market_key, amount).await?;
            println!("Withdraw successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::DepositCollateral { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!(
                "Depositing {} collateral atoms to market {:?}...",
                amount, market_key
            );
            let events = client.deposit_collateral(&market_key, amount).await?;
            println!("Deposit collateral successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::WithdrawCollateral { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!(
                "Withdrawing {:?} collateral atoms from market {:?}...",
                amount, market_key
            );
            let events = client.withdraw_collateral(&market_key, amount).await?;
            println!("Withdraw collateral successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::Borrow { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!("Borrowing {} atoms from market {:?}...", amount, market_key);
            let events = client.borrow(&market_key, amount).await?;
            println!("Borrow successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::Repay { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!("Repaying {:?} atoms to market {:?}...", amount, market_key);
            let events = client.repay(&market_key, amount).await?;
            println!("Repay successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::Liquidate {
            market,
            position,
            max_repay,
            min_collateral,
        } => {
            let market_key = parse_pubkey(&market)?;
            let position_key = parse_pubkey(&position)?;
            println!(
                "Liquidating position {:?} in market {:?}...",
                position_key, market_key
            );
            let events = client
                .liquidate(&market_key, &position_key, max_repay, min_collateral, None)
                .await?;
            println!("Liquidation successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::RedeemCuratorFees { market } => {
            let market_key = parse_pubkey(&market)?;
            println!("Redeeming curator fees from market {:?}...", market_key);
            let events = client.reedeem_curator_fees(&market_key).await?;
            println!("Redeem curator fees successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::RedeemProtocolFees { market } => {
            let market_key = parse_pubkey(&market)?;
            println!("Redeeming protocol fees from market {:?}...", market_key);
            let events = client.reedeem_protocol_fees(&market_key).await?;
            println!("Redeem protocol fees successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::DonateSupply { market, amount } => {
            let market_key = parse_pubkey(&market)?;
            println!("Donating {} atoms to market {:?}...", amount, market_key);
            let events = client.donate_supply(&market_key, amount).await?;
            println!("Donate supply successful!");
            println!("Events: {:#?}", events);
        }

        TxCommands::SocializeLoss { market, authority } => {
            let market_key = parse_pubkey(&market)?;
            let authority_key = parse_pubkey(&authority)?;
            let (position_key, position) = client
                .read_client()
                .get_borrow_position(&market_key, &authority_key);
            if position.is_none() {
                anyhow::bail!(
                    "No borrow position found for authority {:?} in market {:?}",
                    authority_key,
                    market_key
                );
            }
            match client
                .read_client()
                .get_borrow_position_health(&market_key, &authority_key)
            {
                Ok(health) => {
                    println!("=== Position To Socialize ===");
                    println!("Position: {:?}", position_key);
                    println!("LTV: {}", health.ltv);
                    println!("Borrowed Atoms: {}", health.borrowed_atoms);
                    println!("Collateral Atoms: {}", health.collateral_atoms);
                }
                Err(e) => println!("Could not get borrow position health: {}", e),
            }
            println!(
                "Socializing loss for position {:?} in market {:?}...",
                position_key, market_key
            );
            let events = client.socialize_loss(&market_key, &position_key).await?;
            println!("Socialize loss successful!");
            println!("Events: {:#?}", events);
        }
    }
    Ok(())
}

async fn handle_token_command(
    client: &AutaraFullClientWithSigner<
        autara_client::client::single_thread_client::AutaraReadClientImpl,
    >,
    cmd: TokenCommands,
    network: Network,
    signer_keypair: Keypair,
    tn: &TokenNames,
) -> Result<()> {
    let rpc = client.rpc_client();
    let signer_pubkey = *client.signer_pubkey();

    match cmd {
        TokenCommands::CreateToken { decimals } => {
            println!("Creating new token mint with {} decimals...", decimals);

            let (mint_keypair, mint_pubkey, _) = generate_new_keypair(network);
            create_mint_on_chain(
                rpc,
                &signer_pubkey,
                signer_keypair,
                &signer_pubkey,
                mint_keypair,
                &mint_pubkey,
                decimals,
                network,
            )
            .await?;

            println!("Token created successfully!");
            println!("Mint: {:?}", mint_pubkey);
        }

        TokenCommands::Mint { token, to, amount } => {
            let mint_pubkey = parse_pubkey(&token)?;
            let recipient = if let Some(ref to_str) = to {
                parse_pubkey(to_str)?
            } else {
                signer_pubkey
            };

            println!(
                "Minting {} atoms of {} to {:?}...",
                amount,
                tn.name(&mint_pubkey),
                recipient
            );

            let create_ata = create_ata_ix(&signer_pubkey, None, &recipient, &mint_pubkey);
            let recipient_ata = get_associated_token_address(&recipient, &mint_pubkey);

            let mint_to_ix = apl_token::instruction::mint_to(
                &apl_token::id(),
                &mint_pubkey,
                &recipient_ata,
                &signer_pubkey,
                &[],
                amount,
            )?;

            let msg = ArchMessage::new(
                &[create_ata, mint_to_ix],
                Some(signer_pubkey),
                rpc.get_best_block_hash().await?.try_into()?,
            );
            let tx = build_and_sign_transaction(msg, vec![signer_keypair], network)
                .context("Failed to build mint tx")?;
            let txids = rpc.send_transactions(vec![tx]).await?;
            let processed = rpc.wait_for_processed_transactions(txids).await?;
            if processed[0].status != Status::Processed {
                anyhow::bail!(
                    "Failed to mint tokens: {:?}, logs = {:?}",
                    processed[0].status,
                    processed[0].logs
                );
            }

            println!("Mint successful!");
            println!("Recipient ATA: {:?}", recipient_ata);
        }

        TokenCommands::ListAccounts { owner } => {
            let owner_pubkey = if let Some(ref owner_str) = owner {
                parse_pubkey(owner_str)?
            } else {
                signer_pubkey
            };

            println!("=== Token Accounts for {:?} ===", owner_pubkey);
            let balances = rpc.get_all_balances(&owner_pubkey).await?;

            if balances.is_empty() {
                println!("No token accounts found");
            } else {
                for (mint, balance) in &balances {
                    println!("  {}  Balance: {}", tn.name(mint), balance);
                }
                println!("\nTotal token accounts: {}", balances.len());
            }
        }

        TokenCommands::Setup { output } => {
            const TOKEN_AUTHORITY_KEY: &str = "keys/autara-token-authority.key";

            let (_, token_authority_pubkey) =
                with_secret_key_file(TOKEN_AUTHORITY_KEY).context(format!(
                    "Failed to load token authority key: {}",
                    TOKEN_AUTHORITY_KEY
                ))?;

            let tokens = vec![
                TokenDef {
                    name: "BTC",
                    key_file: "keys/token-btc.key",
                    decimals: 8,
                },
                TokenDef {
                    name: "USDC",
                    key_file: "keys/token-usdc.key",
                    decimals: 6,
                },
                TokenDef {
                    name: "ETH",
                    key_file: "keys/token-eth.key",
                    decimals: 8,
                },
            ];

            println!("=== Token Setup ===");
            println!("Token authority: {:?}", token_authority_pubkey);
            let mut config_entries = serde_json::Map::new();

            // Write authority info at the top level
            config_entries.insert(
                "authorityKeyFile".to_string(),
                serde_json::Value::String(TOKEN_AUTHORITY_KEY.to_string()),
            );
            config_entries.insert(
                "authority".to_string(),
                serde_json::Value::String(hex::encode(token_authority_pubkey.serialize())),
            );

            let mut tokens_map = serde_json::Map::new();
            for token_def in &tokens {
                let (mint_keypair, mint_pubkey) = with_secret_key_file(token_def.key_file)
                    .context(format!("Failed to load key file: {}", token_def.key_file))?;

                let already_exists = account_exists(rpc, &mint_pubkey).await;

                if already_exists {
                    println!(
                        "  {} mint already exists: {:?}",
                        token_def.name, mint_pubkey
                    );
                } else {
                    println!(
                        "  Creating {} mint ({} decimals)...",
                        token_def.name, token_def.decimals
                    );
                    create_mint_on_chain(
                        rpc,
                        &signer_pubkey,
                        signer_keypair,
                        &token_authority_pubkey,
                        mint_keypair,
                        &mint_pubkey,
                        token_def.decimals,
                        network,
                    )
                    .await
                    .context(format!("Failed to create {} mint", token_def.name))?;
                    println!("  {} mint created: {:?}", token_def.name, mint_pubkey);
                }

                let mut entry = serde_json::Map::new();
                entry.insert(
                    "mint".to_string(),
                    serde_json::Value::String(hex::encode(mint_pubkey.serialize())),
                );
                entry.insert(
                    "decimals".to_string(),
                    serde_json::Value::Number(token_def.decimals.into()),
                );
                entry.insert(
                    "keyFile".to_string(),
                    serde_json::Value::String(token_def.key_file.to_string()),
                );
                tokens_map.insert(token_def.name.to_string(), serde_json::Value::Object(entry));
            }
            config_entries.insert("tokens".to_string(), serde_json::Value::Object(tokens_map));

            let config = serde_json::Value::Object(config_entries);
            let config_json = serde_json::to_string_pretty(&config)?;
            std::fs::write(&output, &config_json)
                .context(format!("Failed to write config to {}", output))?;
            println!("\nConfig written to {}", output);
            println!("{}", config_json);
        }
    }
    Ok(())
}

struct TokenDef {
    name: &'static str,
    key_file: &'static str,
    decimals: u8,
}

/// Resolves mint pubkeys to human-readable token names from tokens.json
struct TokenNames {
    by_mint: HashMap<Pubkey, String>,
}

impl TokenNames {
    fn load(path: &str) -> Self {
        let mut by_mint = HashMap::new();
        if let Ok(json) = std::fs::read_to_string(path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                if let Some(tokens) = val.get("tokens").and_then(|t| t.as_object()) {
                    for (name, entry) in tokens {
                        if let Some(mint_hex) = entry.get("mint").and_then(|m| m.as_str()) {
                            if let Ok(bytes) = hex::decode(mint_hex) {
                                if bytes.len() == 32 {
                                    let pubkey = Pubkey::from(
                                        <[u8; 32]>::try_from(bytes.as_slice()).unwrap(),
                                    );
                                    by_mint.insert(pubkey, name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        Self { by_mint }
    }

    fn name(&self, pubkey: &Pubkey) -> String {
        match self.by_mint.get(pubkey) {
            Some(name) => format!("{} ({:?})", name, pubkey),
            None => format!("{:?}", pubkey),
        }
    }
}

async fn account_exists(rpc: &arch_sdk::AsyncArchRpcClient, pubkey: &Pubkey) -> bool {
    match rpc.read_account_info(*pubkey).await {
        Ok(_) => true,
        Err(_) => false,
    }
}

async fn create_mint_on_chain(
    rpc: &arch_sdk::AsyncArchRpcClient,
    payer: &Pubkey,
    payer_keypair: Keypair,
    mint_authority: &Pubkey,
    mint_keypair: Keypair,
    mint_pubkey: &Pubkey,
    decimals: u8,
    network: Network,
) -> Result<()> {
    // Step 1: Create the mint account
    let create_msg = ArchMessage::new(
        &[arch_sdk::arch_program::system_instruction::create_account(
            payer,
            mint_pubkey,
            minimum_rent(apl_token::state::Mint::LEN),
            apl_token::state::Mint::LEN as u64,
            &apl_token::id(),
        )],
        Some(*payer),
        rpc.get_best_block_hash().await?.try_into()?,
    );
    let tx = build_and_sign_transaction(create_msg, vec![payer_keypair, mint_keypair], network)
        .context("Failed to build create-account tx")?;
    let txids = rpc.send_transactions(vec![tx]).await?;
    let processed = rpc.wait_for_processed_transactions(txids).await?;
    if processed[0].status != Status::Processed {
        anyhow::bail!(
            "Failed to create mint account: {:?}, logs = {:?}",
            processed[0].status,
            processed[0].logs
        );
    }

    // Step 2: Initialize the mint with dedicated mint authority
    let init_ix = apl_token::instruction::initialize_mint(
        &apl_token::id(),
        mint_pubkey,
        mint_authority,
        Some(mint_authority),
        decimals,
    )?;
    let init_msg = ArchMessage::new(
        &[init_ix],
        Some(*payer),
        rpc.get_best_block_hash().await?.try_into()?,
    );
    let tx = build_and_sign_transaction(init_msg, vec![payer_keypair, mint_keypair], network)
        .context("Failed to build init-mint tx")?;
    let txids = rpc.send_transactions(vec![tx]).await?;
    let processed = rpc.wait_for_processed_transactions(txids).await?;
    if processed[0].status != Status::Processed {
        anyhow::bail!(
            "Failed to initialize mint: {:?}, logs = {:?}",
            processed[0].status,
            processed[0].logs
        );
    }
    Ok(())
}

async fn handle_oracle_command(
    client: &AutaraFullClientWithSigner<
        autara_client::client::single_thread_client::AutaraReadClientImpl,
    >,
    cmd: OracleCommands,
    network: Network,
    signer_keypair: Keypair,
) -> Result<()> {
    let rpc = client.rpc_client();
    let oracle_program_id = autara_oracle_stage_program_id();

    match cmd {
        OracleCommands::FetchPrice { feed } => {
            let feeds: Vec<String> = feed
                .iter()
                .map(|f| {
                    if f.starts_with("0x") {
                        f.clone()
                    } else {
                        format!("0x{}", f)
                    }
                })
                .collect();

            println!("Fetching prices from Pyth API...");
            let result = fetch_pyth_price(&feeds).await?;

            for data in &result.parsed {
                let price_f = data.price.as_float();
                let ema_f = data.ema_price.as_float();
                println!("\nFeed: 0x{}", data.id);
                println!("  Price: {}", price_f);
                println!(
                    "  Confidence: {}",
                    data.price.conf as f64 * 10f64.powi(data.price.expo)
                );
                println!("  EMA Price: {}", ema_f);
                println!("  Expo: {}", data.price.expo);
                println!("  Publish Time: {}", data.price.publish_time);

                let mut feed_id = [0u8; 32];
                hex::decode_to_slice(&data.id, &mut feed_id)
                    .context("Invalid feed ID in response")?;
                let oracle_account = get_pyth_account(&oracle_program_id, feed_id);
                println!("  On-chain Oracle Account: {:?}", oracle_account);
            }
        }

        OracleCommands::PushPrice { feed, price } => {
            let feed_hex = feed.strip_prefix("0x").unwrap_or(&feed);
            let mut feed_id = [0u8; 32];
            hex::decode_to_slice(feed_hex, &mut feed_id).context("Invalid feed ID hex")?;

            println!("Pushing price {} for feed 0x{}...", price, feed_hex);

            let pyth_price = PythPrice::from_dummy(feed_id, price);
            let pusher = AutaraPythPusherClient {
                client: rpc.clone(),
                autara_oracle_program_id: oracle_program_id,
                network,
            };
            pusher
                .push_pyth_price(&signer_keypair, feed_id, &pyth_price)
                .await?;

            let oracle_account = get_pyth_account(&oracle_program_id, feed_id);
            println!("Price pushed successfully!");
            println!("Oracle Account: {:?}", oracle_account);
        }

        OracleCommands::PushFeeds {
            feed,
            push_interval_secs,
        } => {
            let feeds: Vec<String> = feed
                .iter()
                .map(|f| {
                    if f.starts_with("0x") {
                        f.clone()
                    } else {
                        format!("0x{}", f)
                    }
                })
                .collect();

            let push_interval = push_interval_secs
                .map(Duration::from_secs)
                .unwrap_or_else(push_interval_from_env);
            println!(
                "Starting continuous Pyth feed pusher for {} feeds (every {:?})...",
                feeds.len(),
                push_interval
            );
            println!("Press Ctrl+C to stop.");
            for f in &feeds {
                println!("  Feed: {}", f);
            }
            fetch_and_push_feeds(
                rpc,
                &oracle_program_id,
                &signer_keypair,
                &feeds,
                network,
                push_interval,
                None,
            )
            .await;
        }

        OracleCommands::MarketFeeds { market } => {
            let market_key = parse_pubkey(&market)?;
            let market_wrapper = client.read_client().get_market(&market_key);
            if let Some(mw) = market_wrapper {
                let m = mw.market();
                let (supply_oracle_key, collateral_oracle_key) = m.get_oracle_keys();
                println!("=== Oracle Feeds for Market {:?} ===", market_key);
                println!("\nSupply Oracle:");
                println!("  Feed Account: {:?}", supply_oracle_key);
                print_oracle_provider_info(
                    m.supply_vault().oracle_provider().oracle_provider_ref(),
                );
                println!("\nCollateral Oracle:");
                println!("  Feed Account: {:?}", collateral_oracle_key);
                print_oracle_provider_info(
                    m.collateral_vault().oracle_provider().oracle_provider_ref(),
                );
            } else {
                println!("Market not found: {:?}", market_key);
            }
        }
    }
    Ok(())
}

fn print_oracle_provider_info(provider: autara_lib::oracle::oracle_provider::OracleProviderRef) {
    use autara_lib::oracle::oracle_provider::OracleProviderRef;
    match provider {
        OracleProviderRef::Pyth(pyth) => {
            println!("  Type: Pyth");
            println!("  Feed ID: 0x{}", hex::encode(pyth.feed_id));
            println!("  Program ID: {:?}", pyth.program_id);
        }
        OracleProviderRef::Chaos(chaos) => {
            println!("  Type: Chaos");
            println!("  Feed ID: 0x{}", hex::encode(chaos.feed_id));
            println!("  Program ID: {:?}", chaos.program_id);
            println!("  Required Signatures: {}", chaos.required_signatures);
        }
    }
}
