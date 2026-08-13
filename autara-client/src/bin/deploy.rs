use arch_sdk::{arch_program::pubkey::Pubkey, AsyncArchRpcClient, Config};
use autara_client::{
    client::client_with_signer::AutaraFullClientWithSigner,
    config::{autara_oracle_stage_program_id, autara_stage_admin, autara_stage_program_id},
    test::{deploy_new_autara, deploy_new_autara_pyth, AutaraTestEnv},
};

fn main() -> anyhow::Result<()> {
    let config = Config {
        node_endpoint: "".into(),
        node_username: "".into(),
        node_password: "".into(),
        network: arch_sdk::arch_program::bitcoin::Network::Testnet4,
        arch_node_url: "https://rpc.testnet.arch.network".into(),
        titan_url: "".into(),
    };
    deploy_new_autara(&config);
    deploy_new_autara_pyth(&config);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let arch_client = AsyncArchRpcClient::new(&config);
            let test_env = AutaraTestEnv::new(
                arch_client.clone(),
                autara_stage_program_id(),
                autara_oracle_stage_program_id(),
            )
            .await?;
            let admin = autara_stage_admin();
            let pubkey = Pubkey::from_slice(&admin.x_only_public_key().0.serialize());

            arch_client
                .create_and_fund_account_with_faucet(&admin)
                .await?;

            let autara_client = AutaraFullClientWithSigner::new_simple(
                arch_client,
                config.network,
                test_env.autara_program_pubkey,
                test_env.user_keypair,
            );

            autara_client
                .create_global_config(pubkey, pubkey, 5000)
                .await?;
            Ok::<_, anyhow::Error>(())
        })?;
    Ok(())
}
