use arch_sdk::{with_secret_key_file, AsyncArchRpcClient, Config};
use autara_lib::oracle::pyth::{PythPrice, PythPriceAccount};
use autara_pyth::get_pyth_account;
use std::mem::size_of;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let oracle = with_secret_key_file("keys/autara-pyth-stage.key").unwrap().1;
    println!("oracle_program_id={}", oracle);
    println!(
        "sizes: PythPrice={} PythPriceAccount={}",
        size_of::<PythPrice>(),
        size_of::<PythPriceAccount>()
    );

    let config = Config {
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network: arch_program::bitcoin::Network::Testnet,
        titan_url: String::new(),
    };
    let client = AsyncArchRpcClient::new(&config);

    let feeds = [
        (
            "BTC",
            hex::decode("e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43").unwrap(),
        ),
        (
            "USDC",
            hex::decode("eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a").unwrap(),
        ),
    ];

    for (label, id) in feeds {
        let mut feed = [0u8; 32];
        feed.copy_from_slice(&id);
        let pda = get_pyth_account(&oracle, feed);
        match client.read_account_info(pda).await {
            Ok(info) => {
                let layout = if info.data.len() == size_of::<PythPriceAccount>() {
                    "NEW(PythPriceAccount)"
                } else if info.data.len() == size_of::<PythPrice>() {
                    "LEGACY(PythPrice)"
                } else {
                    "UNKNOWN"
                };
                println!(
                    "{label}: pda={pda} owner={} data_len={} layout={layout} lamports={}",
                    info.owner,
                    info.data.len(),
                    info.lamports
                );
            }
            Err(e) => println!("{label}: pda={pda} MISSING err={e}"),
        }
    }

    // also check lending program account exists
    let lending = with_secret_key_file("keys/autara-stage.key").unwrap().1;
    match client.read_account_info(lending).await {
        Ok(info) => println!(
            "lending_program={} executable={} data_len={}",
            lending, info.is_executable, info.data.len()
        ),
        Err(e) => println!("lending_program={lending} err={e}"),
    }
}
