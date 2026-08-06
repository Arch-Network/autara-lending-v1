use std::ops::{Deref, DerefMut};

use arch_program::pubkey::Pubkey;

use crate::{
    error::LendingResult,
    event::{DoubleMarketTransactionEvent, SingleMarketTransactionEvent},
    operation::liquidation::LiquidationResultWithBonus,
    oracle::{oracle_price::OracleRate, oracle_provider::{AccountView, OracleLoader}},
    state::borrow_position::LiquidationResultWithCtx,
};

use super::{
    borrow_position::{BorrowPosition, BorrowPositionHealth},
    market::Market,
    supply_position::SupplyPosition,
};

/// A wrapper around Market to ensure oracles are loaded and validated before any operations
/// requiring oracles
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct MarketWrapper<M> {
    market: M,
    supply_oracle: OracleRate,
    collateral_oracle: OracleRate,
}

impl<M: Deref<Target = Market>> MarketWrapper<M> {
    pub fn try_new<A: Deref<Target = [u8]>, B: Deref<Target = [u8]>>(
        market: M,
        supply_oracle: AccountView<A>,
        collateral_oracle: AccountView<B>,
        unix_timestamp: i64,
    ) -> LendingResult<Self> {
        let supply_oracle = market
            .supply_vault()
            .oracle_config()
            .load_and_validate_oracle_rate(supply_oracle, unix_timestamp)?;
        let collateral_oracle = market
            .collateral_vault()
            .oracle_config()
            .load_and_validate_oracle_rate(collateral_oracle, unix_timestamp)?;
        Ok(Self {
            market,
            supply_oracle,
            collateral_oracle,
        })
    }

    /// Load oracles without timestamp validation (skips staleness and confidence checks)
    pub fn new_unchecked<A: Deref<Target = [u8]>, B: Deref<Target = [u8]>>(
        market: M,
        supply_oracle: AccountView<A>,
        collateral_oracle: AccountView<B>,
    ) -> LendingResult<Self> {
        let supply_oracle = market
            .supply_vault()
            .oracle_config()
            .load_oracle_rate_unchecked(supply_oracle)?;
        let collateral_oracle = market
            .collateral_vault()
            .oracle_config()
            .load_oracle_rate_unchecked(collateral_oracle)?;
        Ok(Self {
            market,
            supply_oracle,
            collateral_oracle,
        })
    }

    /// Try validated oracle loading first; fall back to unchecked if validation fails.
    /// Returns `(wrapper, is_stale)` where `is_stale` is true when unchecked loading was used.
    pub fn try_new_or_unchecked<A: Deref<Target = [u8]>, B: Deref<Target = [u8]>>(
        market: M,
        supply_oracle: AccountView<A>,
        collateral_oracle: AccountView<B>,
        unix_timestamp: i64,
    ) -> LendingResult<(Self, bool)> {
        let supply_config = market.supply_vault().oracle_config();
        let collateral_config = market.collateral_vault().oracle_config();

        let supply_unchecked = supply_config
            .oracle_provider()
            .load_oracle_price(supply_oracle)?;
        let collateral_unchecked = collateral_config
            .oracle_provider()
            .load_oracle_price(collateral_oracle)?;

        let supply_validated = supply_unchecked
            .validate(supply_config.validation_config(), unix_timestamp);
        let collateral_validated = collateral_unchecked
            .validate(collateral_config.validation_config(), unix_timestamp);

        let is_stale = supply_validated.is_err() || collateral_validated.is_err();
        let supply_oracle = supply_validated.unwrap_or_else(|_| supply_unchecked.unsafe_rate());
        let collateral_oracle =
            collateral_validated.unwrap_or_else(|_| collateral_unchecked.unsafe_rate());

        Ok((
            Self {
                market,
                supply_oracle,
                collateral_oracle,
            },
            is_stale,
        ))
    }

    #[inline(always)]
    pub fn market(&self) -> &Market {
        self.market.deref()
    }

    #[inline(always)]
    pub fn collateral_oracle(&self) -> &OracleRate {
        &self.collateral_oracle
    }

    #[inline(always)]
    pub fn supply_oracle(&self) -> &OracleRate {
        &self.supply_oracle
    }

    pub fn borrow_position_health(
        &self,
        borrow_position: &BorrowPosition,
    ) -> LendingResult<BorrowPositionHealth> {
        self.market.borrow_position_health(
            borrow_position,
            &self.collateral_oracle,
            &self.supply_oracle,
        )
    }

    pub fn get_single_market_transaction_event(
        &self,
        market: &Pubkey,
        user: &Pubkey,
        position: &Pubkey,
        mint: &Pubkey,
        atoms: u64,
    ) -> LendingResult<SingleMarketTransactionEvent> {
        let supply_vault_snapshot = self.market.supply_vault().get_summary()?;
        Ok(SingleMarketTransactionEvent {
            market: *market,
            user: *user,
            position: *position,
            mint: *mint,
            amount: atoms,
            supply_vault_summary: supply_vault_snapshot,
            collateral_vault_atoms: self.market.collateral_vault().total_collateral_atoms(),
            supply_oracle_rate: self.supply_oracle,
            collateral_oracle_rate: self.collateral_oracle,
        })
    }

    pub fn get_double_market_transaction_event(
        &self,
        market: &Pubkey,
        user: &Pubkey,
        position: &Pubkey,
        mint_in: &Pubkey,
        amount_in: u64,
        mint_out: &Pubkey,
        amount_out: u64,
    ) -> LendingResult<DoubleMarketTransactionEvent> {
        let supply_vault_snapshot = self.market.supply_vault().get_summary()?;
        Ok(DoubleMarketTransactionEvent {
            market: *market,
            user: *user,
            position: *position,
            mint_in: *mint_in,
            amount_in,
            mint_out: *mint_out,
            amount_out,
            supply_vault_summary: supply_vault_snapshot,
            collateral_vault_atoms: self.market.collateral_vault().total_collateral_atoms(),
            supply_oracle_rate: self.supply_oracle,
            collateral_oracle_rate: self.collateral_oracle,
        })
    }

    pub fn owned(&self) -> MarketWrapper<OwnedMarket> {
        MarketWrapper {
            market: OwnedMarket(self.market.clone()),
            supply_oracle: self.supply_oracle,
            collateral_oracle: self.collateral_oracle,
        }
    }

    pub fn compute_liquidation_result_with_fee(
        &self,
        borrow_position: &BorrowPosition,
        max_repay_atoms: u64,
    ) -> LendingResult<(BorrowPositionHealth, LiquidationResultWithBonus)> {
        self.market.compute_liquidation_result_with_fee(
            borrow_position,
            &self.collateral_oracle,
            &self.supply_oracle,
            max_repay_atoms,
        )
    }
}

impl<M: DerefMut<Target = Market>> MarketWrapper<M> {
    pub fn market_mut(&mut self) -> &mut Market {
        self.market.deref_mut()
    }

    pub fn sync_clock(&mut self, unix_timestamp: i64) -> LendingResult {
        self.market.sync_clock(unix_timestamp)
    }

    pub fn lend(&mut self, supply_position: &mut SupplyPosition, atoms: u64) -> LendingResult {
        self.market.lend(supply_position, atoms)
    }

    pub fn withdraw_collateral(
        &mut self,
        borrow_position: &mut BorrowPosition,
        atoms: u64,
    ) -> LendingResult {
        self.market.withdraw_collateral(
            borrow_position,
            atoms,
            &self.collateral_oracle,
            &self.supply_oracle,
        )
    }

    pub fn deposit_collateral(
        &mut self,
        borrow_position: &mut BorrowPosition,
        atoms: u64,
    ) -> LendingResult {
        self.market.deposit_collateral(borrow_position, atoms)
    }

    pub fn withdraw(&mut self, supply_position: &mut SupplyPosition, atoms: u64) -> LendingResult {
        self.market.withdraw(supply_position, atoms)
    }

    pub fn withdraw_all(&mut self, supply_position: &mut SupplyPosition) -> LendingResult<u64> {
        self.market.withdraw_all(supply_position)
    }

    pub fn borrow(
        &mut self,
        borrow_position: &mut BorrowPosition,
        borrow_atoms: u64,
    ) -> LendingResult {
        self.market.borrow(
            borrow_position,
            borrow_atoms,
            &self.supply_oracle,
            &self.collateral_oracle,
        )
    }

    pub fn repay(&mut self, borrow_position: &mut BorrowPosition, atoms: u64) -> LendingResult {
        self.market.repay(borrow_position, atoms)
    }

    pub fn repay_all(&mut self, borrow_position: &mut BorrowPosition) -> LendingResult<u64> {
        self.market.repay_all(borrow_position)
    }

    pub fn liquidate(
        &mut self,
        borrow_position: &mut BorrowPosition,
        max_repay_atoms: u64,
    ) -> LendingResult<LiquidationResultWithCtx> {
        self.market.liquidate(
            borrow_position,
            &self.collateral_oracle,
            &self.supply_oracle,
            max_repay_atoms,
        )
    }

    pub fn socialize_loss(
        &mut self,
        borrow_position: &mut BorrowPosition,
    ) -> LendingResult<(u64, u64)> {
        self.market.socialize_loss(
            borrow_position,
            &self.collateral_oracle,
            &self.supply_oracle,
        )
    }
}

impl Market {
    pub fn wrapper<A: Deref<Target = [u8]>, B: Deref<Target = [u8]>>(
        &self,
        supply_oracle: AccountView<A>,
        collateral_oracle: AccountView<B>,
        unix_timestamp: i64,
    ) -> LendingResult<MarketWrapper<&Self>> {
        MarketWrapper::try_new(self, supply_oracle, collateral_oracle, unix_timestamp)
    }

    pub fn wrapper_mut<A: Deref<Target = [u8]>, B: Deref<Target = [u8]>>(
        &mut self,
        supply_oracle: AccountView<A>,
        collateral_oracle: AccountView<B>,
        unix_timestamp: i64,
    ) -> LendingResult<MarketWrapper<&mut Self>> {
        MarketWrapper::try_new(self, supply_oracle, collateral_oracle, unix_timestamp)
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct OwnedMarket(pub Market);

impl Deref for OwnedMarket {
    type Target = Market;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for OwnedMarket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
pub mod tests {
    use std::u64;

    use bytemuck::Zeroable;

    use crate::{
        assert_eq_float,
        constant::SECONDS_PER_YEAR,
        error::LendingError,
        oracle::oracle_config::tests::{default_btc_oracle_rate, default_usd_oracle_rate},
        state::{
            collateral_vault::tests::BTC, market::tests::create_empty_btc_usdc_market,
            supply_vault::tests::USDC,
        },
    };

    use super::*;

    fn btc_usd_market() -> MarketWrapper<OwnedMarket> {
        MarketWrapper {
            market: OwnedMarket(create_empty_btc_usdc_market()),
            supply_oracle: default_usd_oracle_rate(),
            collateral_oracle: default_btc_oracle_rate(),
        }
    }

    #[test]
    fn x_deposit_then_withdraw_step_by_step() {
        let mut market = btc_usd_market();
        let mut positions = [SupplyPosition::zeroed(); 5];
        let base = USDC(100_000.);
        for (i, position) in positions.iter_mut().enumerate() {
            let usdc_supplied = (i + 1) as u64 * base;
            market.lend(position, usdc_supplied).unwrap();
            assert_eq!(position.deposited_atoms(), usdc_supplied);
        }
        let supply_snapshot = market.market().supply_vault().get_summary().unwrap();
        assert_eq!(
            supply_snapshot.total_supply,
            base * (positions.len() as u64 * (positions.len() as u64 + 1)) / 2
        );
        for _ in 0..10 {
            for (i, position) in positions.iter_mut().enumerate() {
                let to_withdraw = (i + 1) as u64 * base / 10;
                market.withdraw(position, to_withdraw).unwrap();
            }
        }
        let supply_snapshot = market.market().supply_vault().get_summary().unwrap();
        assert_eq!(supply_snapshot.total_supply, 0);
    }

    #[test]
    fn deposit_borrow_compounds_repay_withdraw() {
        let mut market = btc_usd_market();
        let mut supply_position_one = SupplyPosition::zeroed();
        let mut supply_position_two = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        let usdc_supplied_one = USDC(1_000_000.);
        let usdc_supplied_two = USDC(500_000.);
        market
            .lend(&mut supply_position_one, usdc_supplied_one)
            .unwrap();
        market
            .lend(&mut supply_position_two, usdc_supplied_two)
            .unwrap();
        let btc_collateral = BTC(1.);
        market
            .deposit_collateral(&mut borrow_position, btc_collateral)
            .unwrap();
        market.borrow(&mut borrow_position, USDC(50_000.)).unwrap();
        let utilisation = market
            .market()
            .supply_vault()
            .get_summary()
            .unwrap()
            .utilisation_rate
            .to_float();
        let health = market.borrow_position_health(&borrow_position).unwrap();
        market.sync_clock(SECONDS_PER_YEAR as i64).unwrap();
        let health_after_one_year = market.borrow_position_health(&borrow_position).unwrap();
        assert_eq_float!(
            (health_after_one_year.borrowed_atoms - health.borrowed_atoms) as f64
                / health.borrowed_atoms as f64,
            0.10,   // 10% apy
            0.0001  // 1bps error
        );
        market.repay_all(&mut borrow_position).unwrap();
        let position_one_withdrawn = market.withdraw_all(&mut supply_position_one).unwrap();
        assert_eq_float!(
            (position_one_withdrawn - usdc_supplied_one) as f64 / usdc_supplied_one as f64,
            0.10 * 0.9 * utilisation, // 10% apy with 10% fees
            0.0001                    // 1bps error
        );
        let position_two_withdrawn = market.withdraw_all(&mut supply_position_two).unwrap();
        assert_eq_float!(
            (position_two_withdrawn - usdc_supplied_two) as f64 / usdc_supplied_two as f64,
            0.10 * 0.9 * utilisation, //  10% apy with 10% fees
            0.0001                    // 1bps error
        );
        let curator_fee = market.market_mut().redeem_curator_fess().unwrap() as f64;
        assert_eq_float!(
            curator_fee,
            (position_one_withdrawn - usdc_supplied_one + position_two_withdrawn
                - usdc_supplied_two) as f64
                / 0.9
                * 0.05
        );
        let protocol_fee = market.market_mut().redeem_protocol_fees().unwrap() as f64;
        assert_eq_float!(
            protocol_fee,
            (position_one_withdrawn - usdc_supplied_one + position_two_withdrawn
                - usdc_supplied_two) as f64
                / 0.9
                * 0.05
        );
    }

    #[test]
    pub fn check_cant_liquidated_healthy_position() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        let usdc_supplied = USDC(1_000_000.);
        market.lend(&mut supply_position, usdc_supplied).unwrap();
        let btc_collateral = BTC(1.);
        market
            .deposit_collateral(&mut borrow_position, btc_collateral)
            .unwrap();
        market.borrow(&mut borrow_position, USDC(50_000.)).unwrap();
        assert_eq!(
            market
                .liquidate(&mut borrow_position, USDC(25_000.))
                .unwrap_err(),
            LendingError::PositionIsHealthy
        );
    }

    #[test]
    pub fn check_can_liquidated_unhealthy_position() {
        for oracle_rate in [1, 10, 100, 1000, 10000, 20000, 50000, 55000] {
            let mut market = btc_usd_market();
            let mut supply_position = SupplyPosition::zeroed();
            let mut borrow_position = BorrowPosition::zeroed();
            let usdc_supplied = USDC(1_000_000.);
            market.lend(&mut supply_position, usdc_supplied).unwrap();
            let btc_collateral = BTC(1.);
            market
                .deposit_collateral(&mut borrow_position, btc_collateral)
                .unwrap();
            market.borrow(&mut borrow_position, USDC(50_000.)).unwrap();
            market.collateral_oracle = OracleRate::new(oracle_rate.into(), 0.into());
            market
                .liquidate(&mut borrow_position, USDC(25_000.))
                .unwrap();
        }
    }

    #[test]
    pub fn check_cant_withdraw_more_than_deposited_if_no_compound() {
        for deposit in [1u64, 10, 100, 10u64.pow(5), 10u64.pow(10), 10u64.pow(18)] {
            for initial_supply in [
                1u64,
                20,
                300,
                4 * 10u64.pow(5),
                5 * 10u64.pow(10),
                6 * 10u64.pow(18),
            ] {
                for steps in [1, 2, 3, 10] {
                    let mut market = btc_usd_market();
                    let mut initial_position = SupplyPosition::zeroed();
                    let mut position = SupplyPosition::zeroed();
                    market.lend(&mut initial_position, initial_supply).unwrap();
                    market.lend(&mut position, deposit).unwrap();
                    let withdraw_per_step = deposit / steps;
                    let mut total_withdrawn = 0;
                    for _ in 0..steps - 1 {
                        market.withdraw(&mut position, withdraw_per_step).unwrap();
                        total_withdrawn += withdraw_per_step;
                    }
                    market
                        .withdraw(&mut position, deposit - total_withdrawn)
                        .unwrap();
                    let withdraw = market.withdraw_all(&mut initial_position).unwrap();
                    market.withdraw(&mut position, 1).unwrap_err();
                    assert_eq!(withdraw, initial_supply);
                }
            }
        }
    }

    #[test]
    pub fn check_cant_repay_more_than_borrowed() {
        for borrow_amount in [1u64, 10, 100, 1000] {
            for steps in [1, 2, 3, 10] {
                let mut market = btc_usd_market();
                let mut supply_position = SupplyPosition::zeroed();
                let mut borrow_position = BorrowPosition::zeroed();

                // Provide large liquidity to the market (similar to working test)
                let supply_amount = USDC(1_000_000.);
                market.lend(&mut supply_position, supply_amount).unwrap();

                // Deposit collateral (use same amount as working test)
                let btc_collateral = BTC(1.);
                market
                    .deposit_collateral(&mut borrow_position, btc_collateral)
                    .unwrap();

                // Borrow the test amount (keep it small)
                market
                    .borrow(&mut borrow_position, USDC(borrow_amount as f64))
                    .unwrap();

                // Repay step by step
                let repay_per_step = borrow_amount / steps;
                let mut total_repaid = 0;
                for _ in 0..steps - 1 {
                    market
                        .repay(&mut borrow_position, USDC(repay_per_step as f64))
                        .unwrap();
                    total_repaid += repay_per_step;
                }
                // Repay the remaining amount
                market
                    .repay(
                        &mut borrow_position,
                        USDC((borrow_amount - total_repaid) as f64),
                    )
                    .unwrap();

                // This should fail - trying to repay more than borrowed
                market.repay(&mut borrow_position, USDC(1.)).unwrap_err();
            }
        }
    }

    #[test]
    pub fn cant_socialize_loss_healthy_position() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        let usdc_supplied = USDC(1_000_000.);
        market.lend(&mut supply_position, usdc_supplied).unwrap();
        let btc_collateral = BTC(1.);
        market
            .deposit_collateral(&mut borrow_position, btc_collateral)
            .unwrap();
        market.borrow(&mut borrow_position, USDC(50_000.)).unwrap();
        assert_eq!(
            market.socialize_loss(&mut borrow_position).unwrap_err(),
            LendingError::CannotSocializeDebtForHealthyPosition
        );
    }

    #[test]
    pub fn can_socialize_loss_unhealthy_position() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        let usdc_supplied = USDC(1_000_000.);
        market.lend(&mut supply_position, usdc_supplied).unwrap();
        let btc_collateral = BTC(1.);
        market
            .deposit_collateral(&mut borrow_position, btc_collateral)
            .unwrap();
        market.borrow(&mut borrow_position, USDC(50_000.)).unwrap();
        market.collateral_oracle = OracleRate::new(0.001.into(), 0.into());
        let (collateral_atoms, repaid_atoms) = market.socialize_loss(&mut borrow_position).unwrap();
        assert!(collateral_atoms > 0);
        assert!(repaid_atoms > 0);
    }

    #[test]
    pub fn borrow_near_max_ltv_then_price_drop_triggers_liquidation() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        market.lend(&mut supply_position, USDC(1_000_000.)).unwrap();
        market
            .deposit_collateral(&mut borrow_position, BTC(1.))
            .unwrap();
        // BTC lower_bound = 99,900, max_ltv = 80%, borrow 79k to stay under
        market.borrow(&mut borrow_position, USDC(79_000.)).unwrap();
        let health = market.borrow_position_health(&borrow_position).unwrap();
        assert!(health.ltv < market.market().config().ltv_config().max_ltv);
        // Price drop to 60k makes LTV = 79k/60k = 131% > unhealthy_ltv (90%)
        market.collateral_oracle = OracleRate::new(60_000.into(), 0.into());
        let health_after = market.borrow_position_health(&borrow_position).unwrap();
        assert!(health_after.ltv > market.market().config().ltv_config().unhealthy_ltv);
        market
            .liquidate(&mut borrow_position, USDC(10_000.))
            .unwrap();
    }

    #[test]
    pub fn dust_amounts_dont_break_accounting() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        // Supply 1 atom
        market.lend(&mut supply_position, 10u64.pow(10)).unwrap();
        market.lend(&mut supply_position, 1).unwrap();
        assert_eq!(
            market.market().supply_vault().total_supply().unwrap(),
            10u64.pow(10) + 1
        );
        // Withdraw 1 atom
        market.withdraw(&mut supply_position, 1).unwrap();
        assert_eq!(
            market.market().supply_vault().total_supply().unwrap(),
            10u64.pow(10)
        );
        // Deposit 1 satoshi
        market.deposit_collateral(&mut borrow_position, 1).unwrap();
        assert_eq!(
            market.market().collateral_vault().total_collateral_atoms(),
            1
        );
    }

    #[test]
    pub fn interest_accrual_after_long_period() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        market.lend(&mut supply_position, USDC(1_000_000.)).unwrap();
        market
            .deposit_collateral(&mut borrow_position, BTC(10.))
            .unwrap();
        market.borrow(&mut borrow_position, USDC(100_000.)).unwrap();
        let debt_before = market
            .borrow_position_health(&borrow_position)
            .unwrap()
            .borrowed_atoms;
        // 10 years of interest at 10% APY
        market.sync_clock(10 * SECONDS_PER_YEAR as i64).unwrap();
        let debt_after = market
            .borrow_position_health(&borrow_position)
            .unwrap()
            .borrowed_atoms;
        // After 10 years at 10% APY, debt should be ~2.59x original (1.1^10)
        let ratio = debt_after as f64 / debt_before as f64;
        assert!(ratio > 2.5 && ratio < 2.7);
    }

    #[test]
    pub fn collateral_price_crash_to_near_zero() {
        let mut market = btc_usd_market();
        let mut supply_position = SupplyPosition::zeroed();
        let mut borrow_position = BorrowPosition::zeroed();
        market.lend(&mut supply_position, USDC(1_000_000.)).unwrap();
        market
            .deposit_collateral(&mut borrow_position, BTC(1.))
            .unwrap();
        market.borrow(&mut borrow_position, USDC(50_000.)).unwrap();
        // Price crashes to $1 (99.999% drop)
        market.collateral_oracle = OracleRate::new(1.into(), 0.into());
        // Position is deeply underwater, socialize the loss
        // Returns (debt_atoms, collateral_atoms)
        let (debt_atoms, collateral_atoms) = market.socialize_loss(&mut borrow_position).unwrap();
        assert_eq!(collateral_atoms, BTC(1.));
        assert!(debt_atoms >= USDC(50_000.)); // debt includes any accrued interest
                                              // Supplier takes the loss
        let withdrawn = market.withdraw_all(&mut supply_position).unwrap();
        assert!(withdrawn < USDC(1_000_000.));
    }

    #[test]
    pub fn multiple_borrowers_compete_for_liquidity() {
        let mut market = btc_usd_market();
        let mut supply = SupplyPosition::zeroed();
        let mut borrow1 = BorrowPosition::zeroed();
        let mut borrow2 = BorrowPosition::zeroed();
        market.lend(&mut supply, USDC(100_000.)).unwrap();
        market.deposit_collateral(&mut borrow1, BTC(1.)).unwrap();
        market.deposit_collateral(&mut borrow2, BTC(1.)).unwrap();
        // First borrower takes 50%
        market.borrow(&mut borrow1, USDC(50_000.)).unwrap();
        // Second borrower takes another 40%
        market.borrow(&mut borrow2, USDC(40_000.)).unwrap();
        // Utilization at 90%
        let util = market
            .market()
            .supply_vault()
            .utilisation_rate()
            .unwrap()
            .to_float();
        assert!(util > 0.89 && util < 0.91);
        // Third borrow of 15k should fail (exceeds available)
        let result = market.borrow(&mut borrow1, USDC(15_000.));
        assert!(result.is_err());
    }

    #[test]
    pub fn partial_liquidation_reduces_ltv() {
        let mut market = btc_usd_market();
        let mut supply = SupplyPosition::zeroed();
        let mut borrow = BorrowPosition::zeroed();
        market.lend(&mut supply, USDC(1_000_000.)).unwrap();
        market.deposit_collateral(&mut borrow, BTC(1.)).unwrap();
        market.borrow(&mut borrow, USDC(70_000.)).unwrap();
        // Price drops significantly to trigger liquidation (70k / 75k = 93% > 90%)
        market.collateral_oracle = OracleRate::new(75_000.into(), 0.into());
        let health_before = market.borrow_position_health(&borrow).unwrap();
        assert!(health_before.ltv > market.market().config().ltv_config().unhealthy_ltv);
        // Partial liquidation
        let result = market.liquidate(&mut borrow, USDC(10_000.)).unwrap();
        assert!(result.liquidation_result_with_bonus.borrowed_atoms_to_repay > 0);
        // Position should be healthier after
        let health_after = market.borrow_position_health(&borrow).unwrap();
        assert!(health_after.ltv < health_before.ltv);
    }
}
