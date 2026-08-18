// Operational helper: transfer APL tokens between wallets, creating the
// recipient ATA if missing. Used to pre-fund e2e users on networks without
// mint authorities (mainnet), where e2e_flow runs with E2E_SKIP_MINT=1.
//
// Env: XFER_RPC          Arch JSON-RPC url
//      XFER_NETWORK      mainnet|bitcoin|testnet4
//      XFER_FROM_KEY     signer key file (pays fees + ATA rent)
//      XFER_TO           recipient owner pubkey (hex)
//      XFER_MINT         token mint (hex)
//      XFER_AMOUNT       amount in atoms
//
// Example: XFER_RPC=... XFER_NETWORK=testnet4 XFER_FROM_KEY=... XFER_TO=... \
//          XFER_MINT=... XFER_AMOUNT=1000000 \
//          cargo run -p autara-client --example xfer
use anyhow::{anyhow, Context, Result};
use arch_sdk::{
    arch_program::{bitcoin::Network, pubkey::Pubkey, sanitized::ArchMessage},
    build_and_sign_transaction, with_secret_key_file, AsyncArchRpcClient, Config, Status,
};
use autara_lib::token::{create_ata_ix, get_associated_token_address};

fn pk(h: &str) -> Pubkey {
    Pubkey::from_slice(&hex::decode(h).expect("hex"))
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("missing env {k}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let network = match env("XFER_NETWORK").as_str() {
        "mainnet" | "bitcoin" => Network::Bitcoin,
        "testnet4" => Network::Testnet4,
        other => panic!("unknown XFER_NETWORK={other}"),
    };
    let config = Config {
        arch_node_url: env("XFER_RPC"),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network,
        titan_url: String::new(),
    };
    let rpc = AsyncArchRpcClient::new(&config);

    let (from_kp, from_pk) =
        with_secret_key_file(&env("XFER_FROM_KEY")).map_err(|e| anyhow!("load key: {e}"))?;
    let to = pk(&env("XFER_TO"));
    let mint = pk(&env("XFER_MINT"));
    let amount: u64 = env("XFER_AMOUNT").parse().context("XFER_AMOUNT")?;

    let from_ata = get_associated_token_address(&from_pk, &mint);
    let to_ata = get_associated_token_address(&to, &mint);
    println!("from {} ata {}", hex::encode(from_pk.serialize()), from_ata);
    println!("to   {} ata {}", hex::encode(to.serialize()), to_ata);

    let mut ixs = Vec::new();
    if rpc.read_account_info(to_ata).await.is_err() {
        println!("creating recipient ATA");
        ixs.push(create_ata_ix(&from_pk, None, &to, &mint));
    }
    ixs.push(apl_token::instruction::transfer(
        &apl_token::id(),
        &from_ata,
        &to_ata,
        &from_pk,
        &[],
        amount,
    )?);
    let msg = ArchMessage::new(&ixs, Some(from_pk), rpc.get_best_block_hash().await?);
    let tx = build_and_sign_transaction(msg, vec![from_kp], network)?;
    let txid = rpc.send_transaction(tx).await?;
    let processed = rpc.wait_for_processed_transaction(&txid).await?;
    if !matches!(processed.status, Status::Processed) {
        return Err(anyhow!(
            "transfer FAILED status={:?} logs={:?}",
            processed.status,
            processed.logs
        ));
    }
    println!("TRANSFERRED {amount} atoms -> tx {txid}");
    Ok(())
}
