//! Test-only harness: create an UNDERCOLLATERALIZED borrow position in a single
//! atomic transaction, so it works against the existing testnet markets even
//! while a production price feeder is actively pushing the real price.
//!
//! Trick: the `borrow` instruction checks LTV against the oracle *at execution
//! time*. We bundle, in ONE transaction (instructions execute sequentially and
//! atomically, so the feeder cannot interleave):
//!   [ create_ata? , create_position? , deposit_collateral , PUSH inflated BTC , borrow ]
//! The borrow therefore values our collateral at the inflated price and lets us
//! over-borrow. The instant the tx lands, the feeder's real (lower) price makes
//! the position underwater — and it STAYS underwater on every poll, with the
//! live feeder keeping the oracle fresh (which `liquidate` requires).
//!
//! Usage:
//!   over-borrow --signer keys/<borrower>.key --network testnet \
//!     --market <hex> --deposit-atoms <tBTC atoms> --borrow-atoms <tUSDC atoms> \
//!     --inflated-btc-price 150000

use anyhow::{anyhow, Context, Result};
use arch_sdk::{
    arch_program::{
        account::AccountMeta,
        bitcoin::Network,
        instruction::Instruction,
        pubkey::Pubkey,
        sanitized::ArchMessage,
        system_program::SYSTEM_PROGRAM_ID,
    },
    build_and_sign_transaction, with_secret_key_file, ArchRpcClient, Status,
};
use autara_client::{
    client::{client_with_signer::AutaraFullClientWithSigner, read::AutaraReadClient},
    config::{autara_oracle_stage_program_id, autara_stage_program_id, ArchConfig},
};
use autara_lib::{
    ixs::{borrow_apl_ix, create_borrow_position_ix, deposit_apl_collateral_ix},
    oracle::pyth::PythPrice,
    token::create_ata_ix,
};
use autara_pyth::get_pyth_account;
use clap::Parser;

/// Canonical Pyth BTC/USD feed id (matches the markets' collateral oracle).
const BTC_FEED_HEX: &str = "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43";

#[derive(Parser, Debug)]
#[command(about = "Create an undercollateralized position atomically (test harness)")]
struct Args {
    #[arg(long, default_value = "https://rpc.testnet.arch.network")]
    arch_node: String,
    /// Borrower signer key file (64-char hex secret).
    #[arg(long)]
    signer: String,
    #[arg(long, default_value = "testnet")]
    network: String,
    /// Market pubkey (hex).
    #[arg(long)]
    market: String,
    /// Collateral (tBTC) atoms to deposit in the same tx (0 to skip; position must already hold collateral).
    #[arg(long, default_value_t = 0)]
    deposit_atoms: u64,
    /// Supply (tUSDC) atoms to borrow.
    #[arg(long)]
    borrow_atoms: u64,
    /// Inflated BTC price (USD) to push so the over-borrow passes the max-LTV check.
    #[arg(long, default_value_t = 150_000.0)]
    inflated_btc_price: f64,
    /// Collateral Pyth feed id (hex). Defaults to BTC/USD.
    #[arg(long, default_value = BTC_FEED_HEX)]
    btc_feed: String,
}

fn parse_network(s: &str) -> Result<Network> {
    match s.to_lowercase().as_str() {
        "regtest" => Ok(Network::Regtest),
        "testnet" => Ok(Network::Testnet),
        "mainnet" | "bitcoin" => Ok(Network::Bitcoin),
        other => Err(anyhow!("invalid network: {other}")),
    }
}

fn parse_hex32(s: &str) -> Result<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let v = hex::decode(s).context("invalid hex")?;
    v.try_into().map_err(|_| anyhow!("expected 32 bytes"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();
    let network = parse_network(&args.network)?;
    let market: Pubkey = Pubkey::from(parse_hex32(&args.market)?);
    let btc_feed = parse_hex32(&args.btc_feed)?;

    let config = ArchConfig {
        arch_node_url: args.arch_node.clone(),
        bitcoin_node_endpoint: String::new(),
        bitcoin_node_password: String::new(),
        bitcoin_node_username: String::new(),
    };
    // One client for sending raw txs, one consumed by the read client.
    let send_client: ArchRpcClient = config.arch_rpc_client();

    let (keypair, _) = with_secret_key_file(&args.signer).context("load signer key")?;
    let borrower = Pubkey::from_slice(&keypair.x_only_public_key().0.serialize());

    let autara_program = autara_stage_program_id();
    let oracle_program = autara_oracle_stage_program_id();

    let mut client = AutaraFullClientWithSigner::new_simple(
        config.arch_rpc_client(),
        network,
        autara_program,
        keypair,
    );
    tracing::info!("loading protocol state...");
    client.full_reload().await.context("full_reload")?;

    let mw = client
        .read_client()
        .get_market(&market)
        .context("market not found")?;
    let m = mw.market();
    let supply_mint = m.supply_token_info().mint;
    let collateral_mint = m.collateral_token_info().mint;
    let supply_ata = m.supply_token_info().get_associated_token_address(&borrower);
    let collateral_ata = m
        .collateral_token_info()
        .get_associated_token_address(&borrower);
    let supply_vault = *m.supply_vault().vault();
    let collateral_vault = *m.collateral_vault().vault();
    let (supply_oracle, collateral_oracle) = m.get_oracle_keys();

    // Sanity: the feed id we push must hash to the market's collateral oracle account.
    let derived = get_pyth_account(&oracle_program, btc_feed);
    if derived != collateral_oracle {
        return Err(anyhow!(
            "feed id does not match market collateral oracle: derived {:?} != market {:?}",
            derived,
            collateral_oracle
        ));
    }

    let (borrow_pda, pos) = client
        .read_client()
        .get_borrow_position(&market, &borrower);

    tracing::info!(?borrower, ?market, ?borrow_pda, "borrower / market / position");
    tracing::info!(?supply_mint, ?collateral_mint, ?supply_vault, ?collateral_vault);
    tracing::info!(?supply_oracle, ?collateral_oracle, "oracles");

    let supply_ata_exists = send_client.read_account_info(supply_ata).await.is_ok();

    let mut ixs: Vec<Instruction> = Vec::new();

    // 1) ensure borrower's supply (tUSDC) ATA exists — borrowed funds land here.
    if !supply_ata_exists {
        tracing::info!("creating borrower supply ATA");
        ixs.push(create_ata_ix(&borrower, None, &borrower, &supply_mint));
    }
    // 2) create the borrow position if missing.
    if pos.is_none() {
        tracing::info!("creating borrow position");
        let (_, ix) = create_borrow_position_ix(autara_program, market, borrower, borrower);
        ixs.push(ix);
    }
    // 3) deposit collateral (optional). Collateral ATA must already hold tBTC (minted beforehand).
    if args.deposit_atoms > 0 {
        tracing::info!(deposit_atoms = args.deposit_atoms, "deposit collateral");
        ixs.push(deposit_apl_collateral_ix(
            autara_program,
            market,
            borrower,
            borrow_pda,
            collateral_ata,
            collateral_vault,
            supply_oracle,
            collateral_oracle,
            args.deposit_atoms,
        ));
    }
    // 4) PUSH the inflated BTC price (same tx => borrow below sees it).
    let dummy = PythPrice::from_dummy(btc_feed, args.inflated_btc_price);
    tracing::info!(inflated_btc_price = args.inflated_btc_price, "push inflated collateral price");
    ixs.push(Instruction {
        program_id: oracle_program,
        accounts: vec![
            AccountMeta::new(borrower, true),
            AccountMeta::new(collateral_oracle, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: bytemuck::bytes_of(&dummy).to_vec(),
    });
    // 5) borrow at the inflated valuation.
    tracing::info!(borrow_atoms = args.borrow_atoms, "borrow supply");
    ixs.push(borrow_apl_ix(
        autara_program,
        market,
        borrower,
        borrow_pda,
        supply_ata,
        supply_vault,
        supply_oracle,
        collateral_oracle,
        args.borrow_atoms,
    ));

    // Send all instructions in ONE atomic transaction.
    let blockhash = send_client.get_best_block_hash().await?.try_into()?;
    let message = ArchMessage::new(&ixs, Some(borrower), blockhash);
    let tx = build_and_sign_transaction(message, vec![keypair], network)?;
    let sig = hex::encode(&tx.signatures.first().context("no signature")?.0);
    tracing::info!("sending atomic over-borrow tx: {sig}");
    let txids = send_client.send_transactions(vec![tx]).await?;
    let processed = send_client.wait_for_processed_transactions(txids).await?;
    let result = processed.first().context("no tx processed")?;
    println!("TX: {sig}");
    println!("STATUS: {:?}", result.status);
    if result.status != Status::Processed {
        println!("LOGS: {:?}", result.logs);
        return Err(anyhow!("over-borrow tx failed: {:?}", result.status));
    }
    println!("OK: over-borrowed {} tUSDC atoms against market {}", args.borrow_atoms, args.market);
    println!("Position {borrow_pda:?} should now be unhealthy at the real feeder price.");
    Ok(())
}
