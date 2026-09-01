use crate::{
    error::{LendingError, LendingResult},
    math::{ifixed_point::IFixedPoint, safe_math::SafeMath},
    oracle::oracle_price::OracleRate,
};

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct LiquidationResult {
    pub borrowed_atoms_to_repay: u64,
    pub collateral_atoms_to_liquidate: u64,
}

impl LiquidationResult {
    pub fn adjust_for_max_repay(&mut self, max_repay: u64) {
        if self.borrowed_atoms_to_repay > max_repay {
            self.collateral_atoms_to_liquidate =
                ((self.collateral_atoms_to_liquidate as u128 * max_repay as u128)
                    / self.borrowed_atoms_to_repay as u128) as u64;
            self.borrowed_atoms_to_repay = max_repay;
        }
    }
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct LiquidationResultWithBonus {
    pub borrowed_atoms_to_repay: u64,
    pub collateral_atoms_to_liquidate: u64,
    pub collateral_atoms_liquidation_bonus: u64,
}

impl LiquidationResultWithBonus {
    pub fn adjust_for_max_repay(&mut self, max_repay: u64) {
        if self.borrowed_atoms_to_repay > max_repay {
            let adjust_fn = |value: u64| {
                ((value as u128 * max_repay as u128) / self.borrowed_atoms_to_repay as u128) as u64
            };
            self.collateral_atoms_to_liquidate = adjust_fn(self.collateral_atoms_to_liquidate);
            self.collateral_atoms_liquidation_bonus =
                adjust_fn(self.collateral_atoms_liquidation_bonus);
            self.borrowed_atoms_to_repay = max_repay;
        }
    }

    pub fn total_collateral_atoms_to_liquidate(&self) -> LendingResult<u64> {
        self.collateral_atoms_to_liquidate
            .safe_add(self.collateral_atoms_liquidation_bonus)
    }
}

/// Caller should assert `ltv` and `new_ltv_after_liquidation_fee` < 1
pub fn compute_liquidation_with_fee(
    borrowed_atoms: u64,
    borrow_decimals: u8,
    supply_oracle: &OracleRate,
    collateral_atoms: u64,
    collateral_decimals: u8,
    collateral_oracle: &OracleRate,
    new_ltv_after_liquidation_fee: IFixedPoint,
    liquidation_fee: IFixedPoint,
    max_borrowed_atoms_to_repay: u64,
) -> LendingResult<LiquidationResultWithBonus> {
    let one_plus_liquidation_fee = IFixedPoint::one().safe_add(liquidation_fee)?;
    let adjusted_new_ltv = new_ltv_after_liquidation_fee.safe_mul(one_plus_liquidation_fee)?;
    let adjusted_collateral_atoms = collateral_atoms
        .safe_div(one_plus_liquidation_fee)?
        .as_u64_rounded_down()?;
    let mut liquidation_result = compute_liquidation(
        borrowed_atoms,
        borrow_decimals,
        supply_oracle,
        adjusted_collateral_atoms,
        collateral_decimals,
        collateral_oracle,
        adjusted_new_ltv,
    )?;
    liquidation_result.adjust_for_max_repay(max_borrowed_atoms_to_repay.min(borrowed_atoms));
    // The ltv only stays flat or decreases if the seized share of the collateral is at
    // most the repaid share of the debt. The bonus is part of the seizure, so scaling a
    // full liquidation plan down linearly takes `1 + liquidation_fee` times the repaid
    // share and pushes the ltv *up* as soon as `ltv > 1 / (1 + liquidation_fee)`. Solve
    // for the seizure the repay cap actually affords instead: keep the whole bonus while
    // it fits, and trim it once it no longer does.
    let max_total_collateral_atoms = if liquidation_result.borrowed_atoms_to_repay >= borrowed_atoms
    {
        collateral_atoms
    } else {
        // Raw u128 rather than `SafeMath`: nothing here can fail. A `u64 * u64` product
        // always fits in u128, `borrowed_atoms` is non-zero because the branch above
        // catches `borrowed_atoms_to_repay >= borrowed_atoms` (which a zero
        // `borrowed_atoms` always satisfies), and the quotient cannot exceed
        // `collateral_atoms` because `borrowed_atoms_to_repay < borrowed_atoms` here, so
        // the cast back to u64 is lossless.
        ((collateral_atoms as u128 * liquidation_result.borrowed_atoms_to_repay as u128)
            / borrowed_atoms as u128) as u64
    };
    let mut collateral_atoms_fee = liquidation_result
        .collateral_atoms_to_liquidate
        .safe_mul(liquidation_fee)?
        .as_u64_rounded_down()?;
    let total_collateral_atoms_to_liquidate = liquidation_result
        .collateral_atoms_to_liquidate
        .safe_add(collateral_atoms_fee)?;
    if total_collateral_atoms_to_liquidate > max_total_collateral_atoms {
        // Only reachable for `ltv >= 1`, where the repaid debt is worth more than the
        // collateral backing it, so not even a bonus-free seizure brings the ltv down.
        if liquidation_result.collateral_atoms_to_liquidate > max_total_collateral_atoms {
            return Err(LendingError::LiquidationCannotReduceLtv.into());
        }
        collateral_atoms_fee = max_total_collateral_atoms
            .safe_sub(liquidation_result.collateral_atoms_to_liquidate)?;
    }
    let result = LiquidationResultWithBonus {
        borrowed_atoms_to_repay: liquidation_result.borrowed_atoms_to_repay,
        collateral_atoms_to_liquidate: liquidation_result.collateral_atoms_to_liquidate,
        collateral_atoms_liquidation_bonus: collateral_atoms_fee,
    };
    Ok(result)
}

/// Caller should assert `new_ltv` < `ltv`
///
/// Find `borrowed_atoms_to_repay` and `collateral_atoms_to_liquidate` such that:
///
/// `new_ltv` = (`borrowed_atoms` - `borrowed_atoms_to_repay`) * `supply_oracle` / (`collateral_atoms` - `collateral_atoms_to_liquidate`) * `collateral_oracle`
/// and `borrowed_atoms_to_repay` * `supply_oracle` = `collateral_atoms_to_liquidate` * `collateral_oracle` = `value_to_liquidate`
fn compute_liquidation(
    borrowed_atoms: u64,
    borrow_decimals: u8,
    supply_oracle: &OracleRate,
    collateral_atoms: u64,
    collateral_decimals: u8,
    collateral_oracle: &OracleRate,
    new_ltv: IFixedPoint,
) -> LendingResult<LiquidationResult> {
    let value_to_liquidate = supply_oracle
        .borrow_value(borrowed_atoms, borrow_decimals)?
        .safe_sub(
            collateral_oracle
                .collateral_value(collateral_atoms, collateral_decimals)?
                .safe_mul(new_ltv)?,
        )?
        .safe_div(IFixedPoint::one().safe_sub(new_ltv)?)?;
    let collateral_atoms_to_liquidate =
        collateral_oracle.collateral_atoms(value_to_liquidate, collateral_decimals)?;
    let borrowed_atoms_to_repay =
        supply_oracle.borrow_atoms(value_to_liquidate, borrow_decimals)?;
    Ok(LiquidationResult {
        collateral_atoms_to_liquidate: collateral_atoms_to_liquidate.as_u64_rounded_down()?,
        borrowed_atoms_to_repay: borrowed_atoms_to_repay.as_u64_rounded_up()?,
    })
}

#[cfg(test)]
pub mod tests {
    use std::u64;

    use crate::{
        math::ifixed_point::IFixedPoint,
        oracle::oracle_price::OracleRate,
        state::{
            collateral_vault::tests::{BTC, BTC_DECIMALS},
            supply_vault::tests::{USDC, USDC_DECIMALS},
        },
    };

    use super::*;

    struct LiquidationSetup {
        borrow_decimals: u8,
        supply_oracle: OracleRate,
        collateral_decimals: u8,
        collateral_oracle: OracleRate,
        desired_ltv: IFixedPoint,
    }

    impl LiquidationSetup {
        fn new() -> Self {
            Self {
                borrow_decimals: USDC_DECIMALS as u8,
                supply_oracle: OracleRate::new(IFixedPoint::lit("1"), IFixedPoint::lit("0")),
                collateral_decimals: BTC_DECIMALS as u8,
                collateral_oracle: OracleRate::new(
                    IFixedPoint::lit("100000"),
                    IFixedPoint::lit("0"),
                ),
                desired_ltv: IFixedPoint::lit("0.5"),
            }
        }

        fn calculate_ltv(&self, borrowed_atoms: u64, collateral_atoms: u64) -> IFixedPoint {
            if borrowed_atoms == 0 {
                return IFixedPoint::zero();
            }
            self.supply_oracle
                .borrow_value(borrowed_atoms, self.borrow_decimals)
                .unwrap()
                .safe_div(
                    self.collateral_oracle
                        .collateral_value(collateral_atoms, self.collateral_decimals)
                        .unwrap(),
                )
                .unwrap()
        }

        fn calculate_ltv_after_liquidation(
            &self,
            borrowed_atoms: u64,
            collateral_atoms: u64,
            borrowed_atoms_to_repay: u64,
            collateral_atoms_to_liquidate: u64,
            collateral_atoms_liquidation_fee: u64,
        ) -> IFixedPoint {
            let borrowed_atoms_after_liquidation = borrowed_atoms - borrowed_atoms_to_repay;
            let collateral_atoms_after_liquidation =
                collateral_atoms - collateral_atoms_to_liquidate - collateral_atoms_liquidation_fee;
            self.calculate_ltv(
                borrowed_atoms_after_liquidation,
                collateral_atoms_after_liquidation,
            )
        }

        fn assert_ltv_close_to_target(&self, actual_ltv: IFixedPoint, target_ltv: IFixedPoint) {
            assert!(actual_ltv <= target_ltv);
            assert!(actual_ltv > target_ltv.safe_sub(IFixedPoint::lit("0.000001")).unwrap());
        }
    }

    #[test]
    pub fn check_adjust_for_max_repay() {
        let mut liquidation_result = LiquidationResultWithBonus {
            borrowed_atoms_to_repay: 1000,
            collateral_atoms_to_liquidate: 2000,
            collateral_atoms_liquidation_bonus: 100,
        };
        liquidation_result.adjust_for_max_repay(500);
        assert_eq!(liquidation_result.borrowed_atoms_to_repay, 500);
        assert_eq!(liquidation_result.collateral_atoms_to_liquidate, 1000);
        assert_eq!(liquidation_result.collateral_atoms_liquidation_bonus, 50);
    }

    #[test]
    pub fn check_ltv_post_liquidation() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let LiquidationResult {
            collateral_atoms_to_liquidate,
            borrowed_atoms_to_repay,
        } = compute_liquidation(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
        )
        .unwrap();
        assert_eq!(collateral_atoms_to_liquidate, 60000000);
        assert_eq!(borrowed_atoms_to_repay, 60000000000);
        assert!(borrowed_atoms_to_repay < borrowed_atoms);
        let ltv_after_liquidation = setup.calculate_ltv_after_liquidation(
            borrowed_atoms,
            collateral_atoms,
            borrowed_atoms_to_repay,
            collateral_atoms_to_liquidate,
            0,
        );
        setup.assert_ltv_close_to_target(ltv_after_liquidation, setup.desired_ltv);
    }

    #[test]
    pub fn check_liquidation_with_fee() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let fee = IFixedPoint::lit("0.1");
        let liquidation_result = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            fee,
            u64::MAX,
        )
        .unwrap();
        let ltv_after_liquidation = setup.calculate_ltv_after_liquidation(
            borrowed_atoms,
            collateral_atoms,
            liquidation_result.borrowed_atoms_to_repay,
            liquidation_result.collateral_atoms_to_liquidate,
            liquidation_result.collateral_atoms_liquidation_bonus,
        );
        setup.assert_ltv_close_to_target(ltv_after_liquidation, setup.desired_ltv);
        assert!(liquidation_result.borrowed_atoms_to_repay < borrowed_atoms);
        assert!(
            liquidation_result
                .total_collateral_atoms_to_liquidate()
                .unwrap()
                < collateral_atoms
        );
    }

    #[test]
    pub fn check_full_liquidation_with_reduced_fee() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(90910.);
        let collateral_atoms = BTC(1.);
        let fee = IFixedPoint::lit("0.1");
        let liquidation_result = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            fee,
            u64::MAX,
        )
        .unwrap();
        let ltv_after_liquidation = setup.calculate_ltv_after_liquidation(
            borrowed_atoms,
            collateral_atoms,
            liquidation_result.borrowed_atoms_to_repay,
            liquidation_result.collateral_atoms_to_liquidate,
            liquidation_result.collateral_atoms_liquidation_bonus,
        );
        let liquidation_fee =
            liquidation_result.collateral_atoms_liquidation_bonus as f64 / collateral_atoms as f64;
        assert!(liquidation_fee < fee.to_float() - 0.0001);
        assert!(ltv_after_liquidation.is_zero());
        assert!(liquidation_result.borrowed_atoms_to_repay == borrowed_atoms);
        assert!(
            liquidation_result
                .total_collateral_atoms_to_liquidate()
                .unwrap()
                == collateral_atoms
        );
    }

    #[test]
    pub fn liquidation_reduces_ltv() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let ltv_before = setup.calculate_ltv(borrowed_atoms, collateral_atoms);
        let result = compute_liquidation(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
        )
        .unwrap();
        let ltv_after = setup.calculate_ltv_after_liquidation(
            borrowed_atoms,
            collateral_atoms,
            result.borrowed_atoms_to_repay,
            result.collateral_atoms_to_liquidate,
            0,
        );
        assert!(ltv_after < ltv_before);
    }

    #[test]
    pub fn total_collateral_never_exceeds_available() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let fee = IFixedPoint::lit("0.1");
        let result = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            fee,
            u64::MAX,
        )
        .unwrap();
        assert!(result.total_collateral_atoms_to_liquidate().unwrap() <= collateral_atoms);
    }

    #[test]
    pub fn repay_never_exceeds_borrowed() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let result = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            IFixedPoint::lit("0.05"),
            u64::MAX,
        )
        .unwrap();
        assert!(result.borrowed_atoms_to_repay <= borrowed_atoms);
    }

    #[test]
    pub fn max_repay_respected() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let max_repay = USDC(10000.);
        let result = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            IFixedPoint::lit("0.05"),
            max_repay,
        )
        .unwrap();
        assert!(result.borrowed_atoms_to_repay <= max_repay);
    }

    /// At `ltv >= 1` the repaid debt is worth more than the collateral behind it, so no
    /// seizure the repay cap affords reduces the ltv, not even a bonus-free one. That is
    /// reported as `LiquidationCannotReduceLtv` rather than as an opaque arithmetic error
    /// from subtracting a larger seizure from a smaller cap.
    #[test]
    pub fn partial_liquidation_of_insolvent_position_is_rejected() {
        let setup = LiquidationSetup::new();
        // 1 BTC at 100k backing 120k of debt: ltv = 1.2.
        let borrowed_atoms = USDC(120_000.);
        let collateral_atoms = BTC(1.);
        let error = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            IFixedPoint::lit("0.1"),
            USDC(10_000.),
        )
        .unwrap_err();
        assert_eq!(error, LendingError::LiquidationCannotReduceLtv);
    }

    #[test]
    pub fn bonus_proportional_to_collateral() {
        let setup = LiquidationSetup::new();
        let borrowed_atoms = USDC(80000.);
        let collateral_atoms = BTC(1.);
        let fee = IFixedPoint::lit("0.05");
        let result = compute_liquidation_with_fee(
            borrowed_atoms,
            setup.borrow_decimals,
            &setup.supply_oracle,
            collateral_atoms,
            setup.collateral_decimals,
            &setup.collateral_oracle,
            setup.desired_ltv,
            fee,
            u64::MAX,
        )
        .unwrap();
        if result.collateral_atoms_to_liquidate > 0 {
            let actual_fee_ratio = result.collateral_atoms_liquidation_bonus as f64
                / result.collateral_atoms_to_liquidate as f64;
            assert!(actual_fee_ratio <= fee.to_float() + 0.001);
        }
    }

    #[test]
    pub fn adjust_max_repay_scales_proportionally() {
        let mut result = LiquidationResultWithBonus {
            borrowed_atoms_to_repay: 1000,
            collateral_atoms_to_liquidate: 500,
            collateral_atoms_liquidation_bonus: 50,
        };
        let ratio_before =
            result.collateral_atoms_to_liquidate as f64 / result.borrowed_atoms_to_repay as f64;
        result.adjust_for_max_repay(500);
        let ratio_after =
            result.collateral_atoms_to_liquidate as f64 / result.borrowed_atoms_to_repay as f64;
        assert!((ratio_before - ratio_after).abs() < 0.01);
    }

    #[test]
    pub fn adjust_max_repay_no_change_if_under_limit() {
        let mut result = LiquidationResultWithBonus {
            borrowed_atoms_to_repay: 500,
            collateral_atoms_to_liquidate: 250,
            collateral_atoms_liquidation_bonus: 25,
        };
        let original = result;
        result.adjust_for_max_repay(1000);
        assert_eq!(
            result.borrowed_atoms_to_repay,
            original.borrowed_atoms_to_repay
        );
        assert_eq!(
            result.collateral_atoms_to_liquidate,
            original.collateral_atoms_to_liquidate
        );
    }

    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        fn liquidation_setup() -> LiquidationSetup {
            LiquidationSetup::new()
        }

        proptest! {
            #[test]
            fn liquidation_always_reduces_ltv(
                borrowed_usdc in 60_000u64..95_000u64,
            ) {
                let setup = liquidation_setup();
                let borrowed_atoms = USDC(borrowed_usdc as f64);
                let collateral_atoms = BTC(1.);
                let ltv_before = setup.calculate_ltv(borrowed_atoms, collateral_atoms);
                if ltv_before > setup.desired_ltv {
                    let result = compute_liquidation(
                        borrowed_atoms,
                        setup.borrow_decimals,
                        &setup.supply_oracle,
                        collateral_atoms,
                        setup.collateral_decimals,
                        &setup.collateral_oracle,
                        setup.desired_ltv,
                    ).unwrap();
                    if result.borrowed_atoms_to_repay > 0 {
                        let ltv_after = setup.calculate_ltv_after_liquidation(
                            borrowed_atoms, collateral_atoms,
                            result.borrowed_atoms_to_repay, result.collateral_atoms_to_liquidate, 0,
                        );
                        prop_assert!(ltv_after < ltv_before);
                    }
                }
            }

            #[test]
            fn max_repay_always_respected(
                borrowed_usdc in 60_000u64..90_000u64,
                max_repay_usdc in 1_000u64..50_000u64,
            ) {
                let setup = liquidation_setup();
                let borrowed_atoms = USDC(borrowed_usdc as f64);
                let collateral_atoms = BTC(1.);
                let max_repay = USDC(max_repay_usdc as f64);
                let fee = IFixedPoint::lit("0.05");
                let result = compute_liquidation_with_fee(
                    borrowed_atoms, setup.borrow_decimals, &setup.supply_oracle,
                    collateral_atoms, setup.collateral_decimals, &setup.collateral_oracle,
                    setup.desired_ltv, fee, max_repay,
                ).unwrap();
                prop_assert!(result.borrowed_atoms_to_repay <= max_repay);
            }

            #[test]
            fn total_collateral_liquidated_never_exceeds_available(
                borrowed_usdc in 50_000u64..99_000u64,
                fee_thousandths in 1u64..100u64,
            ) {
                let setup = liquidation_setup();
                let borrowed_atoms = USDC(borrowed_usdc as f64);
                let collateral_atoms = BTC(1.);
                let fee = IFixedPoint::from_ratio(fee_thousandths, 1000).unwrap();
                if fee <= IFixedPoint::lit("0.1") && fee >= IFixedPoint::lit("0.001") {
                    let result = compute_liquidation_with_fee(
                        borrowed_atoms, setup.borrow_decimals, &setup.supply_oracle,
                        collateral_atoms, setup.collateral_decimals, &setup.collateral_oracle,
                        setup.desired_ltv, fee, u64::MAX,
                    ).unwrap();
                    prop_assert!(result.total_collateral_atoms_to_liquidate().unwrap() <= collateral_atoms);
                }
            }

            #[test]
            fn adjust_for_max_repay_preserves_ratio(
                repay in 1_000u64..1_000_000u64,
                collateral in 500u64..500_000u64,
                bonus in 10u64..50_000u64,
                max_repay_pct in 10u64..90u64,
            ) {
                let mut result = LiquidationResultWithBonus {
                    borrowed_atoms_to_repay: repay,
                    collateral_atoms_to_liquidate: collateral,
                    collateral_atoms_liquidation_bonus: bonus,
                };
                let ratio_before = collateral as f64 / repay as f64;
                let max_repay = repay * max_repay_pct / 100;
                if max_repay > 0 {
                    result.adjust_for_max_repay(max_repay);
                    prop_assert!(result.borrowed_atoms_to_repay <= max_repay);
                    if result.borrowed_atoms_to_repay > 0 {
                        let ratio_after = result.collateral_atoms_to_liquidate as f64 / result.borrowed_atoms_to_repay as f64;
                        prop_assert!((ratio_before - ratio_after).abs() < 0.01);
                    }
                }
            }

            #[test]
            fn repay_never_exceeds_borrowed(
                borrowed_usdc in 60_000u64..95_000u64,
            ) {
                let setup = liquidation_setup();
                let borrowed_atoms = USDC(borrowed_usdc as f64);
                let collateral_atoms = BTC(1.);
                let fee = IFixedPoint::lit("0.05");
                let result = compute_liquidation_with_fee(
                    borrowed_atoms, setup.borrow_decimals, &setup.supply_oracle,
                    collateral_atoms, setup.collateral_decimals, &setup.collateral_oracle,
                    setup.desired_ltv, fee, u64::MAX,
                ).unwrap();
                prop_assert!(result.borrowed_atoms_to_repay <= borrowed_atoms);
            }

            /// The invariant the seizure cap exists to keep: a liquidation holds the ltv
            /// flat or lower only when the seized share of the collateral is at most the
            /// repaid share of the debt. With `B` the debt and `C` the collateral, that is
            /// `seized / C <= repay / B`, asserted here in exact integer form so no
            /// fixed-point rounding can mask a violation.
            ///
            /// Regression: folding the bonus into the seizure and then scaling the plan
            /// down linearly seized `1 + fee` times the repaid share, breaking this for
            /// every `ltv > 1 / (1 + fee)`. The borrowed range spans ltv 0.6 to 0.99, so it
            /// crosses that threshold for the larger fees in range.
            #[test]
            fn seized_share_never_exceeds_repaid_share(
                borrowed_usdc in 60_000u64..99_000u64,
                max_repay_pct in 1u64..=100u64,
                fee_thousandths in 1u64..=100u64,
            ) {
                let setup = liquidation_setup();
                let borrowed_atoms = USDC(borrowed_usdc as f64);
                let collateral_atoms = BTC(1.);
                let fee = IFixedPoint::from_ratio(fee_thousandths, 1000).unwrap();
                let max_repay = borrowed_atoms / 100 * max_repay_pct;
                match compute_liquidation_with_fee(
                    borrowed_atoms, setup.borrow_decimals, &setup.supply_oracle,
                    collateral_atoms, setup.collateral_decimals, &setup.collateral_oracle,
                    setup.desired_ltv, fee, max_repay,
                ) {
                    Ok(result) => {
                        let seized = result.total_collateral_atoms_to_liquidate().unwrap();
                        prop_assert!(
                            seized as u128 * borrowed_atoms as u128
                                <= result.borrowed_atoms_to_repay as u128
                                    * collateral_atoms as u128,
                            "seized {}/{} exceeds repaid {}/{}",
                            seized,
                            collateral_atoms,
                            result.borrowed_atoms_to_repay,
                            borrowed_atoms,
                        );
                    }
                    // The only refusal this path may produce. Every ltv generated here is
                    // below 1, so reaching it would itself be the bug.
                    Err(e) => prop_assert_eq!(e, LendingError::LiquidationCannotReduceLtv),
                }
            }
        }
    }
}
