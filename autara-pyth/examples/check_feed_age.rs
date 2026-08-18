use arch_sdk::{with_secret_key_file, AsyncArchRpcClient, Config};
use autara_lib::oracle::pyth::PythPriceAccount;
use autara_pyth::get_pyth_account;
use std::mem::size_of;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let oracle = with_secret_key_file("keys/autara-pyth-stage.key").unwrap().1;
    let config = Config {
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        node_endpoint: String::new(),
        node_username: String::new(),
        node_password: String::new(),
        network: arch_program::bitcoin::Network::Testnet,
        titan_url: String::new(),
    };
    let client = AsyncArchRpcClient::new(&config);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let feeds = [
        ("BTC", "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43"),
        ("USDC", "eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a"),
    ];
    for (label, hex_id) in feeds {
        let id = hex::decode(hex_id).unwrap();
        let mut feed = [0u8; 32];
        feed.copy_from_slice(&id);
        let pda = get_pyth_account(&oracle, feed);
        let info = client.read_account_info(pda).await.expect("account");
        assert_eq!(info.data.len(), size_of::<PythPriceAccount>());
        let acc: &PythPriceAccount = bytemuck::from_bytes(&info.data);
        let age = now - acc.pyth_price.price.publish_time;
        println!(
            "{label}: publish_time={} age_secs={} authority={} price={}",
            acc.pyth_price.price.publish_time,
            age,
            hex::encode(acc.authority.serialize()),
            acc.pyth_price.price.price
        );
    }
    // expected pusher pubkey
    let expected = "c7c43936060721b9dc04e927968afa0cebd528f3182865e33a8563835f1e435b";
    println!("expected_pusher_pubkey={expected}");
    match client
        .read_account_info(arch_program::pubkey::Pubkey::from_slice(
            &hex::decode(expected).unwrap(),
        ))
        .await
    {
        Ok(a) => println!("pusher_lamports={}", a.lamports),
        Err(e) => println!("pusher_account_err={e}"),
    }
}
