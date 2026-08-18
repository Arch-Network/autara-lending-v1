use anyhow::{Context, Result};
use arch_sdk::AsyncArchRpcClient;
use autara_client::client::{read::AutaraReadClient, single_thread_client::AutaraReadClientImpl};
use autara_liquidator::config::{LiquidatorConfig, TokenFilter, parse_hex_pubkey};
use clap::Parser;

#[derive(Parser)]
#[command(name = "liquidator-check-exposure")]
#[command(about = "Read-only liquidation inventory sizing from current lending positions")]
struct Args {
    #[arg(long, default_value = "liquidator-config.json")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config: LiquidatorConfig = serde_json::from_str(
        &std::fs::read_to_string(&args.config).context("failed to read config")?,
    )
    .context("failed to parse config")?;
    let filter = TokenFilter::from_config(&config.restrict_tokens)?;
    let network = config.parse_network()?;
    let rpc = AsyncArchRpcClient::new(&arch_sdk::Config {
        arch_node_url: config.rpc_url,
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    });
    let mut client = AutaraReadClientImpl::new(
        rpc,
        parse_hex_pubkey(&config.autara_program_id).context("invalid lending program ID")?,
    );
    client
        .reload()
        .await
        .context("failed to load lending state")?;

    let mut position_count = 0usize;
    let mut unhealthy_count = 0usize;
    let mut total_borrowed_atoms = 0u128;
    let mut largest_borrowed_atoms = 0u64;
    let mut maximum_immediate_repay_atoms = 0u64;
    for (position, borrow) in client.all_borrow_position() {
        let market_key = borrow.market();
        let Some(market) = client.get_market(market_key) else {
            continue;
        };
        let supply_mint = market.market().supply_token_info().mint;
        let collateral_mint = market.market().collateral_token_info().mint;
        if !filter.allows_market(&supply_mint, &collateral_mint) {
            continue;
        }
        let health = market
            .borrow_position_health(&borrow)
            .context("failed to calculate borrow health")?;
        position_count += 1;
        total_borrowed_atoms += u128::from(health.borrowed_atoms);
        largest_borrowed_atoms = largest_borrowed_atoms.max(health.borrowed_atoms);
        let unhealthy_ltv = market.market().config().ltv_config().unhealthy_ltv;
        if health.ltv >= unhealthy_ltv {
            unhealthy_count += 1;
            let (_, liquidation) = market
                .market()
                .compute_liquidation_result_with_fee(
                    &borrow,
                    market.collateral_oracle(),
                    market.supply_oracle(),
                    u64::MAX,
                )
                .context("failed to preview liquidation")?;
            maximum_immediate_repay_atoms =
                maximum_immediate_repay_atoms.max(liquidation.borrowed_atoms_to_repay);
        }
        println!(
            "position={position:?} market={market_key:?} supply_mint={supply_mint:?} collateral_mint={collateral_mint:?} borrowed_atoms={} collateral_atoms={} ltv={} unhealthy={}",
            health.borrowed_atoms,
            health.collateral_atoms,
            health.ltv,
            health.ltv >= unhealthy_ltv,
        );
    }
    println!("position_count: {position_count}");
    println!("unhealthy_count: {unhealthy_count}");
    println!("total_borrowed_atoms: {total_borrowed_atoms}");
    println!("largest_borrowed_atoms: {largest_borrowed_atoms}");
    println!("maximum_immediate_repay_atoms: {maximum_immediate_repay_atoms}");
    Ok(())
}
