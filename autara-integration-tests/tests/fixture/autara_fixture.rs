use arch_sdk::{
    arch_program::{bitcoin::key::Keypair, pubkey::Pubkey},
    ArchRpcClient,
};
use autara_client::{
    client::{
        client_with_signer::AutaraFullClientWithSigner, single_thread_client::AutaraReadClientImpl,
    },
    config::{
        autara_oracle_stage_program_id, autara_stage_admin, autara_stage_program_id, ArchConfig,
    },
    rpc_ext::ArchAsyncRpcExt,
    test::AutaraTestEnv,
    token_mint::TokenMint,
};
use autara_lib::{
    interest_rate::interest_rate_kind::InterestRateCurveKind, ixs::CreateMarketInstruction,
    math::ifixed_point::IFixedPoint, state::market_config::LtvConfig,
};

use crate::fixture::balance::Balance;

pub struct AutaraFixture {
    user_client: AutaraFullClientWithSigner<AutaraReadClientImpl>,
    test_env: AutaraTestEnv,
    admin: Keypair,
}

pub const LTV: IFixedPoint = IFixedPoint::from_i64_u64_ratio(8, 10);
pub const UNHEALTHY_LTV: IFixedPoint = IFixedPoint::from_i64_u64_ratio(9, 10);
pub const LIQUIDATION_BONUS: IFixedPoint = IFixedPoint::from_i64_u64_ratio(5, 100);
pub const MAX_UTILISATION_RATE: IFixedPoint = IFixedPoint::from_i64_u64_ratio(9, 10);

impl AutaraFixture {
    pub async fn new() -> Self {
        let config = ArchConfig::testnet();
        let arch_client = config.arch_rpc_client();
        let admin = autara_stage_admin();
        let env = AutaraTestEnv::new(
            arch_client.clone(),
            autara_stage_program_id(),
            autara_oracle_stage_program_id(),
        )
        .await
        .unwrap();
        let this = Self::from_env(arch_client, env, admin);
        tokio::try_join!(
            this.env().push_collateral_price(100_000.),
            this.env().push_supply_price(1.)
        )
        .unwrap();
        return this;
    }

    fn from_env(arch_client: ArchRpcClient, test_env: AutaraTestEnv, admin: Keypair) -> Self {
        let user_client = AutaraFullClientWithSigner::new_simple(
            arch_client,
            arch_sdk::arch_program::bitcoin::Network::Regtest,
            test_env.autara_program_pubkey,
            test_env.user_keypair,
        );
        AutaraFixture {
            test_env,
            user_client,
            admin,
        }
    }

    pub fn env(&self) -> &AutaraTestEnv {
        &self.test_env
    }

    pub fn curator_client(&self) -> AutaraFullClientWithSigner<&AutaraReadClientImpl> {
        self.user_client
            .with_signer(self.test_env.authority_keypair)
    }

    pub fn user_client(&self) -> &AutaraFullClientWithSigner<AutaraReadClientImpl> {
        &self.user_client
    }

    pub fn user_two_client(&self) -> AutaraFullClientWithSigner<&AutaraReadClientImpl> {
        self.user_client
            .with_signer(self.test_env.user_two_keypair.clone())
    }

    pub fn admin_client(&self) -> AutaraFullClientWithSigner<&AutaraReadClientImpl> {
        self.user_client.with_signer(self.admin.clone())
    }

    pub async fn reload(&mut self) {
        self.user_client
            .full_reload()
            .await
            .expect("Failed to reload user client");
    }

    pub async fn reload_market(&mut self, market: &Pubkey) {
        self.user_client
            .reload_authority_accounts_for_market(market)
            .await
            .expect("Failed to reload user client");
    }

    pub async fn fetch_balance(&self, user: &Pubkey) -> Balance {
        let balance = self
            .user_client
            .rpc_client()
            .get_balances(
                user,
                &[
                    TokenMint::new(self.test_env.collateral_mint, 0),
                    TokenMint::new(self.test_env.supply_mint, 0),
                ],
            )
            .await
            .expect("Failed to fetch balances");
        Balance {
            supply: balance
                .get(&self.test_env.supply_mint)
                .copied()
                .unwrap_or_default(),
            collateral: balance
                .get(&self.test_env.collateral_mint)
                .copied()
                .unwrap_or_default(),
        }
    }

    pub async fn fetch_user_balance(&self) -> Balance {
        self.fetch_balance(self.user_client.signer_pubkey()).await
    }

    pub async fn create_market(&mut self) -> Pubkey {
        let m = self
            .curator_client()
            .create_market(
                CreateMarketInstruction {
                    market_bump: 0,
                    index: 0,
                    ltv_config: LtvConfig {
                        max_ltv: LTV,
                        unhealthy_ltv: UNHEALTHY_LTV,
                        liquidation_bonus: LIQUIDATION_BONUS,
                    },
                    max_utilisation_rate: MAX_UTILISATION_RATE,
                    supply_oracle_config: self.env().supply_oracle_config(),
                    collateral_oracle_config: self.env().collateral_oracle_config(),
                    interest_rate: InterestRateCurveKind::new_adaptive(),
                    lending_market_fee_in_bps: 100,
                },
                self.env().supply_mint,
                self.env().collateral_mint,
            )
            .await
            .unwrap();
        self.reload().await;
        m
    }
}

#[allow(non_snake_case)]
pub const fn BTC(amount: f64) -> u64 {
    (amount * 100_000_000.0) as u64
}

#[allow(non_snake_case)]
pub const fn USDC(amount: f64) -> u64 {
    (amount * 100_000_000.0) as u64
}
