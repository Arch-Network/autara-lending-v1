use arch_sdk::{
    arch_program::{
        bitcoin::{self, key::Keypair},
        program_pack::Pack,
        pubkey::Pubkey,
        rent::minimum_rent,
        sanitized::ArchMessage,
    },
    build_and_sign_transaction, generate_new_keypair, with_secret_key_file, ArchRpcClient,
    AsyncArchRpcClient, Config, ProgramDeployer, ProgramDeployerError, Status,
};
use autara_lib::{
    oracle::{oracle_config::OracleConfig, pyth::PythPrice},
    token::{create_ata_ix, get_associated_token_address},
};
use autara_pyth::{fetch_and_push_feeds, AutaraPythPusherClient};

use crate::config::path_from_workspace;

pub const NODE1_ADDRESS: &str = "http://localhost:9002/";
pub const BITCOIN_NETWORK: bitcoin::Network = bitcoin::Network::Testnet;

pub fn deploy_program(config: &Config, deployer: Option<&str>, key: &str, path: &str) -> Pubkey {
    let (program_keypair, pubkey) = with_secret_key_file(&path_from_workspace(key)).unwrap();
    let (authority_keypair, _) = deployer.map_or_else(
        || {
            let (k, pk, _) = generate_new_keypair(config.network);
            (k, pk)
        },
        |owner| with_secret_key_file(&path_from_workspace(owner)).unwrap(),
    );
    let client = ArchRpcClient::new(config);
    for _ in 0..2 {
        client
            .create_and_fund_account_with_faucet(&authority_keypair)
            .expect("create and fund account with faucet should not fail");
    }
    if let Err(err) = ProgramDeployer::new(config).try_deploy_program(
        path.to_string(),
        program_keypair,
        authority_keypair,
        &path_from_workspace(path),
    ) {
        if let ProgramDeployerError::TransactionError(msg) = &err {
            if msg.contains("already exists") {
                return pubkey;
            }
        }
        panic!("Failed to deploy program: {:?}", err);
    };
    pubkey
}

pub fn deploy_new_autara(config: &Config) -> Pubkey {
    deploy_program(
        config,
        Some("keys/autara-deployer.key"),
        "keys/autara-stage.key",
        "target/deploy/autara_program.so",
    )
}

pub fn deploy_new_autara_pyth(config: &Config) -> Pubkey {
    deploy_program(
        config,
        Some("keys/autara-deployer.key"),
        "keys/autara-pyth-stage.key",
        "target/deploy/autara_oracle.so",
    )
}

#[derive(Clone)]
pub struct AutaraTestEnv {
    pub arch_client: AsyncArchRpcClient,
    pub autara_program_pubkey: Pubkey,
    pub autara_oracle_program_pubkey: Pubkey,
    pub authority_keypair: Keypair,
    pub user_keypair: Keypair,
    pub user_pubkey: Pubkey,
    pub user_two_keypair: Keypair,
    pub user_two_pubkey: Pubkey,
    pub supply_mint: Pubkey,
    pub supply_minter: TokenMinter,
    pub supply_feed_id: [u8; 32],
    pub collateral_mint: Pubkey,
    pub collateral_minter: TokenMinter,
    pub collateral_feed_id: [u8; 32],
}

impl AutaraTestEnv {
    pub async fn new(
        arch_client: AsyncArchRpcClient,
        autara_program_pubkey: Pubkey,
        autara_oracle_program_pubkey: Pubkey,
    ) -> anyhow::Result<Self> {
        let (authority_keypair, authority, _) = generate_new_keypair(BITCOIN_NETWORK);
        let (user_keypair, user_one_pubkey, _) = generate_new_keypair(BITCOIN_NETWORK);
        let (user_two_keypair, user_two_pubkey, _) = generate_new_keypair(BITCOIN_NETWORK);
        tokio::try_join!(
            arch_client.create_and_fund_account_with_faucet(&user_keypair),
            arch_client.create_and_fund_account_with_faucet(&user_two_keypair),
            arch_client.create_and_fund_account_with_faucet(&authority_keypair)
        )?;
        let amounts = [
            (user_one_pubkey, 1 << 55),
            (user_two_pubkey, 1 << 55),
            (authority, 1 << 55),
        ];
        let (supply_minter, collateral_minter) = tokio::try_join!(
            TokenMinter::new(arch_client.clone(), authority_keypair.clone(), &amounts),
            TokenMinter::new(arch_client.clone(), authority_keypair.clone(), &amounts)
        )?;
        Ok(Self {
            arch_client,
            autara_program_pubkey,
            autara_oracle_program_pubkey,
            authority_keypair,
            user_keypair,
            user_two_keypair,
            collateral_feed_id: collateral_minter.mint_pubkey().0,
            supply_feed_id: supply_minter.mint_pubkey().0,
            supply_mint: supply_minter.mint_pubkey(),
            supply_minter,
            collateral_mint: collateral_minter.mint_pubkey(),
            collateral_minter,
            user_pubkey: user_one_pubkey,
            user_two_pubkey,
        })
    }

    pub fn supply_oracle_config(&self) -> OracleConfig {
        OracleConfig::new_pyth(self.supply_feed_id, self.autara_oracle_program_pubkey)
    }

    pub fn collateral_oracle_config(&self) -> OracleConfig {
        OracleConfig::new_pyth(self.collateral_feed_id, self.autara_oracle_program_pubkey)
    }

    pub async fn push_supply_price(&self, price: f64) -> anyhow::Result<()> {
        let pyth = PythPrice::from_dummy(self.supply_feed_id, price);
        AutaraPythPusherClient {
            client: self.arch_client.clone(),
            autara_oracle_program_id: self.autara_oracle_program_pubkey,
            network: BITCOIN_NETWORK,
        }
        .push_pyth_price(&self.authority_keypair, self.supply_feed_id, &pyth)
        .await
    }

    pub async fn push_collateral_price(&self, price: f64) -> anyhow::Result<()> {
        let pyth = PythPrice::from_dummy(self.collateral_feed_id, price);
        AutaraPythPusherClient {
            client: self.arch_client.clone(),
            autara_oracle_program_id: self.autara_oracle_program_pubkey,
            network: BITCOIN_NETWORK,
        }
        .push_pyth_price(&self.authority_keypair, self.collateral_feed_id, &pyth)
        .await
    }

    pub async fn push_price(&self, feed_id: [u8; 32], price: f64) -> anyhow::Result<()> {
        let pyth = PythPrice::from_dummy(feed_id, price);
        AutaraPythPusherClient {
            client: self.arch_client.clone(),
            autara_oracle_program_id: self.autara_oracle_program_pubkey,
            network: BITCOIN_NETWORK,
        }
        .push_pyth_price(&self.authority_keypair, feed_id, &pyth)
        .await
    }

    pub fn spawn_pyth_pusher(&self) {
        let client = self.arch_client.clone();
        let authority = self.authority_keypair.clone();
        let autara_oracle_program_id = self.autara_oracle_program_pubkey;
        let feeds = [
            hex::encode(self.supply_feed_id),
            hex::encode(self.collateral_feed_id),
        ];
        tokio::spawn(async move {
            fetch_and_push_feeds(
                &client,
                &autara_oracle_program_id,
                &authority,
                &feeds,
                BITCOIN_NETWORK,
                autara_pyth::push_interval_from_env(),
                None,
            )
            .await
        });
    }
}

#[derive(Clone)]
pub struct TokenMinter {
    client: AsyncArchRpcClient,
    authority_keypair: Keypair,
    authority_pubkey: Pubkey,
    mint_pubkey: Pubkey,
}

impl TokenMinter {
    pub async fn new(
        client: AsyncArchRpcClient,
        authority_keypair: Keypair,
        users: &[(Pubkey, u64)],
    ) -> anyhow::Result<Self> {
        let authority_pubkey =
            Pubkey::from_slice(&authority_keypair.x_only_public_key().0.serialize());
        let mint = create_mint_and_mint_custom_amounts(&client, authority_keypair, users).await?;
        Ok(Self {
            client,
            authority_keypair,
            authority_pubkey,
            mint_pubkey: mint,
        })
    }

    pub fn from_existing(
        client: AsyncArchRpcClient,
        authority_keypair: Keypair,
        mint_pubkey: Pubkey,
    ) -> Self {
        let authority_pubkey =
            Pubkey::from_slice(&authority_keypair.x_only_public_key().0.serialize());
        Self {
            client,
            authority_keypair,
            authority_pubkey,
            mint_pubkey,
        }
    }

    pub fn mint_pubkey(&self) -> Pubkey {
        self.mint_pubkey
    }

    pub async fn mint_to(&self, user: &Pubkey, amount: u64) -> anyhow::Result<()> {
        let user_ata = get_associated_token_address(user, &self.mint_pubkey);
        let ata_exists = self.client.read_account_info(user_ata).await.is_ok();

        let mint_ix = apl_token::instruction::mint_to(
            &apl_token::id(),
            &self.mint_pubkey,
            &user_ata,
            &self.authority_pubkey,
            &[],
            amount,
        )?;

        let mut ixs = Vec::new();
        if !ata_exists {
            ixs.push(create_ata_ix(
                &self.authority_pubkey,
                None,
                user,
                &self.mint_pubkey,
            ));
        }
        ixs.push(mint_ix);

        let message = ArchMessage::new(
            &ixs,
            Some(self.authority_pubkey),
            self.client.get_best_block_hash().await?.try_into()?,
        );
        let signers = vec![self.authority_keypair];
        let txid = build_and_sign_transaction(message, signers, BITCOIN_NETWORK)
            .expect("Failed to build and sign transaction");
        let txids = self.client.send_transactions(vec![txid]).await?;
        let processed_tx = self.client.wait_for_processed_transactions(txids).await?;
        if processed_tx[0].status != Status::Processed {
            return Err(anyhow::anyhow!(
                "Failed to mint tokens: {:?}, logs = {:?}",
                processed_tx[0].status,
                processed_tx[0].logs
            ));
        }
        Ok(())
    }

    pub async fn credit_to(&self, user: &Pubkey, amount: u64) -> anyhow::Result<()> {
        let create_user_ata = create_ata_ix(&self.authority_pubkey, None, user, &self.mint_pubkey);
        let transfer = apl_token::instruction::transfer(
            &apl_token::id(),
            &get_associated_token_address(&self.authority_pubkey, &self.mint_pubkey),
            &create_user_ata.accounts[1].pubkey,
            &self.authority_pubkey,
            &[&self.authority_pubkey],
            amount,
        )?;
        let message = ArchMessage::new(
            &[create_user_ata, transfer],
            Some(self.authority_pubkey),
            self.client.get_best_block_hash().await?.try_into()?,
        );
        let signers = vec![self.authority_keypair];
        let txid = build_and_sign_transaction(message, signers, BITCOIN_NETWORK)
            .expect("Failed to build and sign transaction");
        let txids = self.client.send_transactions(vec![txid]).await?;
        let processed_tx = self.client.wait_for_processed_transactions(txids).await?;
        if processed_tx[0].status != Status::Processed {
            return Err(anyhow::anyhow!(
                "Failed to mint tokens: {:?}, logs = {:?}",
                processed_tx[0].status,
                processed_tx[0].logs
            ));
        }
        Ok(())
    }
}

pub async fn create_mint_and_mint_custom_amounts(
    client: &AsyncArchRpcClient,
    authority_and_payer_keypair: Keypair,
    users_and_amounts: &[(Pubkey, u64)],
) -> anyhow::Result<Pubkey> {
    let payer = Pubkey::from_slice(
        &authority_and_payer_keypair
            .x_only_public_key()
            .0
            .serialize(),
    );

    // Generate new mint keypair
    let (mint_keypair, mint_pubkey, _) = generate_new_keypair(BITCOIN_NETWORK);

    // Step 1: Create the mint account
    let create_account_message = ArchMessage::new(
        &[arch_sdk::arch_program::system_instruction::create_account(
            &payer,
            &mint_pubkey,
            minimum_rent(apl_token::state::Mint::LEN),
            apl_token::state::Mint::LEN as u64,
            &apl_token::id(),
        )],
        Some(payer),
        client.get_best_block_hash().await?.try_into()?,
    );

    let create_account_signers = vec![authority_and_payer_keypair, mint_keypair];
    let create_account_txid = build_and_sign_transaction(
        create_account_message,
        create_account_signers,
        BITCOIN_NETWORK,
    )
    .unwrap();

    let txids = client.send_transactions(vec![create_account_txid]).await?;
    let processed_tx = client.wait_for_processed_transactions(txids).await?;

    if processed_tx[0].status != Status::Processed {
        return Err(anyhow::anyhow!(
            "Failed to create mint account: {:?}",
            processed_tx[0].status
        ));
    }

    let mut instructions = Vec::new();

    // Initialize the mint
    instructions.push(apl_token::instruction::initialize_mint(
        &apl_token::id(),
        &mint_pubkey,
        &payer,
        Some(&payer),
        9, // decimals
    )?);

    for (user, amount) in users_and_amounts {
        let create_user_ata = create_ata_ix(&payer, None, user, &mint_pubkey);

        let user_ata = get_associated_token_address(user, &mint_pubkey);

        instructions.push(create_user_ata);

        instructions.push(apl_token::instruction::mint_to(
            &apl_token::id(),
            &mint_pubkey,
            &user_ata,
            &payer,
            &[],
            *amount,
        )?);
    }

    let initialize_message = ArchMessage::new(
        &instructions,
        Some(payer),
        client.get_best_block_hash().await?.try_into()?,
    );

    let initialize_signers = vec![authority_and_payer_keypair, mint_keypair];
    let initialize_txid =
        build_and_sign_transaction(initialize_message, initialize_signers, BITCOIN_NETWORK)
            .unwrap();

    let txids = client.send_transactions(vec![initialize_txid]).await?;
    let processed_tx = client.wait_for_processed_transactions(txids).await?;

    if processed_tx[0].status != Status::Processed {
        return Err(anyhow::anyhow!(
            "Failed to initialize mint and mint tokens: {:?}, logs = {:?}",
            processed_tx[0].status,
            processed_tx[0].logs
        ));
    }
    Ok(mint_pubkey)
}
