use arch_sdk::{
    arch_program::{bitcoin::key::Keypair, pubkey::Pubkey},
    with_secret_key_file,
};

pub const BITCOIN_NODE_ENDPOINT: &str = "http://127.0.0.1:18443/wallet/testwallet";
pub const BITCOIN_NODE_USERNAME: &str = "bitcoin";
pub const BITCOIN_NODE_PASSWORD: &str = "bitcoinpass";
pub const NODE1_ADDRESS: &str = "http://localhost:9002/";

pub struct ArchConfig {
    pub arch_node_url: String,
    pub bitcoin_node_endpoint: String,
    pub bitcoin_node_password: String,
    pub bitcoin_node_username: String,
}

impl ArchConfig {
    pub fn load_from_env() -> Self {
        Self {
            arch_node_url: std::env::var("ARCH_NODE_URL").expect("ARCH_NODE_URL must be set"),
            bitcoin_node_endpoint: std::env::var("BITCOIN_NODE_ENDPOINT")
                .expect("BITCOIN_NODE_ENDPOINT must be set"),
            bitcoin_node_password: std::env::var("BITCOIN_NODE_PASSWORD")
                .expect("BITCOIN_NODE_PASSWORD must be set"),
            bitcoin_node_username: std::env::var("BITCOIN_NODE_USERNAME")
                .expect("BITCOIN_NODE_USERNAME must be set"),
        }
    }

    pub fn dev() -> Self {
        Self {
            bitcoin_node_endpoint: BITCOIN_NODE_ENDPOINT.to_string(),
            arch_node_url: NODE1_ADDRESS.to_string(),
            bitcoin_node_password: BITCOIN_NODE_PASSWORD.to_string(),
            bitcoin_node_username: BITCOIN_NODE_USERNAME.to_string(),
        }
    }

    pub fn testnet() -> Self {
        Self {
            bitcoin_node_endpoint:
                "https://bitcoin-node.test.aws.archnetwork.xyz:49332/wallet/testwallet".to_string(),
            arch_node_url: "https://rpc.testnet.arch.network".into(),
            bitcoin_node_password: "bitcoin".to_string(),
            bitcoin_node_username: "uU1taFBTUvae96UCtA8YxAepYTFszYvYVSXK8xgzBs0".to_string(),
        }
    }

    pub fn arigato() -> Self {
        Self {
            bitcoin_node_endpoint: "http://100.101.31.18:18332".to_string(),
            arch_node_url: "http://100.101.31.18:9002".to_string(),
            bitcoin_node_password: "arigatobitcoin".to_string(),
            bitcoin_node_username: "arigatonode0bitcoin".to_string(),
        }
    }

    pub fn arch_rpc_client(&self) -> arch_sdk::ArchRpcClient {
        let config = arch_sdk::Config {
            arch_node_url: self.arch_node_url.clone(),
            node_endpoint: self.bitcoin_node_endpoint.clone(),
            node_username: self.bitcoin_node_username.clone(),
            node_password: self.bitcoin_node_password.clone(),
            network: arch_sdk::arch_program::bitcoin::Network::Regtest,
            titan_url: String::new(),
        };
        arch_sdk::ArchRpcClient::new(&config)
    }

    pub async fn bitcoin_rpc_client(
        &self,
    ) -> bitcoincore_rpc_async::Result<bitcoincore_rpc_async::Client> {
        bitcoincore_rpc_async::Client::new(
            self.bitcoin_node_endpoint.clone(),
            bitcoincore_rpc_async::Auth::UserPass(
                self.bitcoin_node_username.clone(),
                self.bitcoin_node_password.clone(),
            ),
        )
        .await
    }
}

pub fn path_from_workspace(path: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../{}", path))
        .as_path()
        .to_str()
        .unwrap()
        .to_string()
}

pub fn file_to_pubkey(path: &str) -> Pubkey {
    with_secret_key_file(path).unwrap().1
}

pub fn autara_stage_program_id() -> Pubkey {
    file_to_pubkey(&path_from_workspace("keys/autara-stage.key"))
}

pub fn autara_oracle_stage_program_id() -> Pubkey {
    file_to_pubkey(&path_from_workspace("keys/autara-pyth-stage.key"))
}

pub fn autara_stage_admin() -> Keypair {
    with_secret_key_file(&path_from_workspace("keys/autara-admin-stage.key"))
        .unwrap()
        .0
}
