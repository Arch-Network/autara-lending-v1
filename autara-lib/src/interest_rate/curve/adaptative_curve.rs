use borsh::{BorshDeserialize, BorshSerialize};
use bytemuck::{Pod, Zeroable};

use crate::constant::SECONDS_PER_YEAR;
use crate::interest_rate::interest_rate_per_second::InterestRatePerSecond;
use crate::math::safe_math::SafeMath;
use crate::{
    error::LendingResult, interest_rate::lending_interest_rate::MarketBorrowRateParameters,
    math::ifixed_point::IFixedPoint,
};

/// Autara rust version of the AdaptiveCurveIrm used in [Morpho](https://github.com/morpho-org/morpho-blue-irm/blob/main/src/adaptive-curve-irm/AdaptiveCurveIrm.sol)
///
/// NOTE :
/// If a market using this curve stays at extreme
/// utilization (near 100%) and no transaction calls `sync_clock` for ~1.1+ years, the
/// `linear_adaptation` value can exceed the domain of `checked_exp()` (~55.26), causing
/// `new_rate_at_target` to permanently fail. Because `last_update_unix_timestamp` is only
/// advanced on success, subsequent calls accumulate even larger elapsed times and also fail.
/// In practice this is extremely unlikely since any supply/borrow/repay/liquidation triggers
/// `sync_clock`, and the market would need to be completely idle at >90% utilization for over
/// a year. A PoC test exists in `state/mod.rs::poc_adaptive_curve_can_perma_brick_market_after_long_idle`.
#[repr(C)]
#[derive(Debug, PartialEq, Eq, Copy, Clone, Pod, Zeroable, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct AdaptiveInterestRateCurve {
    rate_at_target: InterestRatePerSecond,
}

const TARGET_UTILISATION_RATE: IFixedPoint = IFixedPoint::from_i64_u64_ratio(9, 10);
const ONE_MINUS_TARGET_UTILISATION_RATE: IFixedPoint = IFixedPoint::from_i64_u64_ratio(1, 10);
const ADJUSTMENT_SPEED: IFixedPoint = IFixedPoint::from_i64_u64_ratio(50, SECONDS_PER_YEAR);
const CURVE_STEEPNESS: u64 = 4;

const INITIAL_RATE_AT_TARGET: InterestRatePerSecond =
    InterestRatePerSecond::const_from_apr(IFixedPoint::from_i64_u64_ratio(4, 100));
const MIN_RATE_AT_TARGET: InterestRatePerSecond =
    InterestRatePerSecond::const_from_apr(IFixedPoint::from_i64_u64_ratio(1, 100));
const MAX_RATE_AT_TARGET: InterestRatePerSecond =
    InterestRatePerSecond::const_from_apr(IFixedPoint::from_i64_u64_ratio(200, 100));

/// Safe clamp bounds for `linear_adaptation` passed to `checked_exp()`.
/// checked_exp domain: args ≤ -33.21 → Ok(0), args > 55.26 → Err. We clamp to those exact bounds.
/// Fix for issue #47: prevents permanent market brick at extreme utilization after long idle.
const MAX_SAFE_LINEAR_ADAPTATION: IFixedPoint = IFixedPoint::from_i64_u64_ratio(5526, 100);
const MIN_SAFE_LINEAR_ADAPTATION: IFixedPoint = IFixedPoint::from_i64_u64_ratio(-3321, 100);

impl AdaptiveInterestRateCurve {
    pub fn new() -> Self {
        AdaptiveInterestRateCurve {
            rate_at_target: InterestRatePerSecond::const_from_apr(IFixedPoint::zero()),
        }
    }

    pub fn borrow_rate(
        &mut self,
        params: MarketBorrowRateParameters,
    ) -> LendingResult<InterestRatePerSecond> {
        let (rate_at_target, end_rate_at_target) = self.compute_next_rates(params)?;
        self.rate_at_target = end_rate_at_target;
        Ok(rate_at_target)
    }

    fn compute_next_rates(
        &self,
        params: MarketBorrowRateParameters,
    ) -> LendingResult<(InterestRatePerSecond, InterestRatePerSecond)> {
        let err_norm_factor = if params.utilisation_rate > &TARGET_UTILISATION_RATE {
            ONE_MINUS_TARGET_UTILISATION_RATE
        } else {
            TARGET_UTILISATION_RATE
        };
        let err = params
            .utilisation_rate
            .safe_sub(TARGET_UTILISATION_RATE)?
            .safe_div(err_norm_factor)?;
        let start_rate_at_target = self.rate_at_target;
        let avg_rate_at_target;
        let end_rate_at_target;
        if start_rate_at_target.is_zero() {
            avg_rate_at_target = INITIAL_RATE_AT_TARGET;
            end_rate_at_target = INITIAL_RATE_AT_TARGET;
        } else {
            let speed = ADJUSTMENT_SPEED.safe_mul(err)?;
            let linear_adaptation = speed.safe_mul(params.elapsed_seconds_since_last_update)?;
            if linear_adaptation.is_zero() {
                avg_rate_at_target = start_rate_at_target;
                end_rate_at_target = start_rate_at_target;
            } else {
                end_rate_at_target =
                    Self::new_rate_at_target(start_rate_at_target, linear_adaptation)?;
                let mid_rate_at_target =
                    Self::new_rate_at_target(start_rate_at_target, linear_adaptation.safe_div(2)?)?;
                avg_rate_at_target = InterestRatePerSecond::new(
                    (start_rate_at_target
                        .safe_add(end_rate_at_target.0)?
                        .safe_add(mid_rate_at_target.safe_mul(2)?)?)
                    .safe_div(4)?,
                );
            }
        }
        let rate_at_target = Self::curve(avg_rate_at_target, err)?;
        Ok((rate_at_target, end_rate_at_target))
    }

    fn curve(
        rate_at_target: InterestRatePerSecond,
        err: IFixedPoint,
    ) -> LendingResult<InterestRatePerSecond> {
        let coeff = if err.is_negative() {
            IFixedPoint::one().safe_sub(IFixedPoint::one().safe_div(CURVE_STEEPNESS)?)?
        } else {
            CURVE_STEEPNESS.safe_sub(IFixedPoint::one())?
        };
        coeff
            .safe_mul(err)?
            .safe_add(IFixedPoint::one())?
            .safe_mul(rate_at_target.0)
            .map(InterestRatePerSecond)
    }

    fn new_rate_at_target(
        start_rate_at_target: InterestRatePerSecond,
        linear_adaptation: IFixedPoint,
    ) -> LendingResult<InterestRatePerSecond> {
        // Clamp linear_adaptation to the safe domain of checked_exp (±55).
        // Without this, a market idle at ≥90% utilization for ~1.1+ years permanently fails:
        // checked_exp returns Err, last_update_unix_timestamp never advances, and every
        // subsequent call accumulates more elapsed time — bricking the market forever.
        let clamped = if linear_adaptation > MAX_SAFE_LINEAR_ADAPTATION {
            MAX_SAFE_LINEAR_ADAPTATION
        } else if linear_adaptation < MIN_SAFE_LINEAR_ADAPTATION {
            MIN_SAFE_LINEAR_ADAPTATION
        } else {
            linear_adaptation
        };
        start_rate_at_target
            .safe_mul(clamped.checked_exp()?)
            .map(|x| InterestRatePerSecond::new(x).clamp(MIN_RATE_AT_TARGET, MAX_RATE_AT_TARGET))
    }
}

#[cfg(test)]
/// Morpho tests : https://github.com/morpho-org/morpho-blue-irm/blob/main/test/forge/AdaptiveCurveIrmTest.sol
pub mod tests {
    use super::*;
    use crate::{assert_eq_float, constant::SECONDS_PER_DAY};

    fn days_to_seconds(days: u64) -> u64 {
        days * SECONDS_PER_DAY
    }

    #[test]
    fn test_first_borrow_utilisation_zero() {
        let mut curve = AdaptiveInterestRateCurve::new();
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(0, 1),
            elapsed_seconds_since_last_update: days_to_seconds(90),
        };
        assert_eq_float!(
            curve.borrow_rate(params).unwrap().to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float() / 4.
        );
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float()
        );
    }

    #[test]
    fn test_first_borrow_utilisation_one() {
        let mut curve = AdaptiveInterestRateCurve::new();
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(1, 1),
            elapsed_seconds_since_last_update: days_to_seconds(90),
        };
        assert_eq_float!(
            curve.borrow_rate(params).unwrap().to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float() * 4.
        );
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float()
        );
    }

    #[test]
    fn test_rate_after_utilisation_one() {
        let mut curve = AdaptiveInterestRateCurve::new();
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(0, 1),
            elapsed_seconds_since_last_update: days_to_seconds(90 + 2 * 365),
        };
        assert_eq_float!(
            curve.borrow_rate(params).unwrap().to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float() / 4.
        );
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(1, 1),
            elapsed_seconds_since_last_update: days_to_seconds(5),
        };
        let rate = curve.compute_next_rates(params).unwrap().0.to_float();
        assert_eq_float!(
            rate,
            (INITIAL_RATE_AT_TARGET.0.to_float() * 4.)
                * ((1.9836 - 1.) / (ADJUSTMENT_SPEED.to_float() * days_to_seconds(5) as f64)),
            0.05
        );
        assert_eq_float!(
            rate,
            (INITIAL_RATE_AT_TARGET.0.to_float() * 4.) * 1.4361,
            0.05
        );
        assert_eq_float!(rate, 0.22976 / days_to_seconds(365) as f64, 0.05);
    }

    #[test]
    fn test_rate_after_utilisation_zero() {
        let mut curve = AdaptiveInterestRateCurve::new();
        // First call to establish initial state (2 years elapsed)
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(0, 1),
            elapsed_seconds_since_last_update: days_to_seconds(2 * 365),
        };
        assert_eq_float!(
            curve.borrow_rate(params).unwrap().to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float() / 4.
        );

        // Second call with 5 days elapsed and zero utilization
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(0, 1),
            elapsed_seconds_since_last_update: days_to_seconds(5),
        };
        let rate = curve.compute_next_rates(params).unwrap().0.to_float();

        // exp((-50/365)*5) ≈ 0.5041, so the coefficient is (0.5041 - 1) / (-50/365 * 5) ≈ 0.724
        assert_eq_float!(
            rate,
            (INITIAL_RATE_AT_TARGET.0.to_float() / 4.) * 0.724,
            0.1
        );
        // Expected rate: 0.724% per year
        assert_eq_float!(rate, 0.00724 / days_to_seconds(365) as f64, 0.1);
    }

    #[test]
    fn test_rate_after_45_days_utilisation_above_target_no_ping() {
        let mut curve = AdaptiveInterestRateCurve::new();

        // First establish the curve at target utilization
        let params = MarketBorrowRateParameters {
            utilisation_rate: &TARGET_UTILISATION_RATE,
            elapsed_seconds_since_last_update: days_to_seconds(1),
        };
        let rate = curve.borrow_rate(params).unwrap();
        assert_eq_float!(rate.to_float(), INITIAL_RATE_AT_TARGET.0.to_float());
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float()
        );

        // Now test 45 days later with utilization above target (error = 50%)
        // Utilization = (TARGET_UTILIZATION + 1.0) / 2 = (0.9 + 1.0) / 2 = 0.95
        let params = MarketBorrowRateParameters {
            utilisation_rate: &IFixedPoint::from_i64_u64_ratio(95, 100),
            elapsed_seconds_since_last_update: days_to_seconds(45),
        };
        curve.borrow_rate(params).unwrap();

        // Expected rate: 4% * exp(50 * 45 / 365 * 50%) = 87.22%
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            0.8722 / days_to_seconds(365) as f64,
            0.005
        );
    }

    #[test]
    fn test_rate_after_45_days_utilisation_above_target_ping_every_minute() {
        let mut curve = AdaptiveInterestRateCurve::new();

        // First establish the curve at target utilization
        let params = MarketBorrowRateParameters {
            utilisation_rate: &TARGET_UTILISATION_RATE,
            elapsed_seconds_since_last_update: days_to_seconds(1),
        };
        let rate = curve.borrow_rate(params).unwrap();
        assert_eq_float!(rate.to_float(), INITIAL_RATE_AT_TARGET.0.to_float());

        // Initial borrow assets with utilization at 95% (error = 50%)
        let mut total_supply_assets = IFixedPoint::one();
        let mut total_borrow_assets = IFixedPoint::from_i64_u64_ratio(95, 100);

        // Simulate 45 days of minute-by-minute updates
        let minutes_in_45_days = 45 * 24 * 60;
        for _ in 0..minutes_in_45_days {
            let utilisation_rate = total_borrow_assets.safe_div(total_supply_assets).unwrap();
            let params = MarketBorrowRateParameters {
                utilisation_rate: &utilisation_rate,
                elapsed_seconds_since_last_update: 60, // 1 minute
            };

            let avg_borrow_rate = curve.borrow_rate(params).unwrap();

            // Calculate compound interest for 1 minute
            let interest_factor = avg_borrow_rate
                .0
                .safe_mul(IFixedPoint::from_i64_u64_ratio(60, 1))
                .unwrap();
            let interest = total_borrow_assets.safe_mul(interest_factor).unwrap();

            total_supply_assets = total_supply_assets.safe_add(interest).unwrap();
            total_borrow_assets = total_borrow_assets.safe_add(interest).unwrap();
        }

        // Check final utilization is approximately 95%
        let final_utilization = total_borrow_assets.safe_div(total_supply_assets).unwrap();
        assert_eq_float!(final_utilization.to_float(), 0.95, 0.01);

        // Expected rate: 4% * exp(50 * 45 / 365 * 50%) = 87.22%
        let expected_rate_at_target = 0.8722 / days_to_seconds(365) as f64;
        assert!(curve.rate_at_target.to_float() >= expected_rate_at_target);
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            expected_rate_at_target,
            0.08 // 8% tolerance due to minute-by-minute pings
        );

        // Expected growth: exp(87.22% * 3.5 * 45 / 365) = +45.70%
        let initial_borrow_assets = IFixedPoint::from_i64_u64_ratio(95, 100);
        let expected_final_borrow_assets = initial_borrow_assets
            .safe_mul(IFixedPoint::from_i64_u64_ratio(1457, 1000))
            .unwrap();
        assert_eq_float!(
            total_borrow_assets.to_float(),
            expected_final_borrow_assets.to_float(),
            0.3 // 30% tolerance due to minute-by-minute pings
        );
    }

    #[test]
    fn test_rate_after_utilisation_target_no_ping() {
        // Test with various elapsed times
        let test_cases = [
            0,
            days_to_seconds(1),
            days_to_seconds(30),
            days_to_seconds(365),
            days_to_seconds(1000),
        ];

        for elapsed in test_cases {
            let mut curve = AdaptiveInterestRateCurve::new();

            // First establish the curve at target utilization
            let params = MarketBorrowRateParameters {
                utilisation_rate: &TARGET_UTILISATION_RATE,
                elapsed_seconds_since_last_update: days_to_seconds(1),
            };
            let rate = curve.borrow_rate(params).unwrap();
            assert_eq_float!(rate.to_float(), INITIAL_RATE_AT_TARGET.0.to_float());
            assert_eq_float!(
                curve.rate_at_target.to_float(),
                INITIAL_RATE_AT_TARGET.0.to_float()
            );

            // Test after elapsed time at target utilization
            let params = MarketBorrowRateParameters {
                utilisation_rate: &TARGET_UTILISATION_RATE,
                elapsed_seconds_since_last_update: elapsed,
            };
            curve.borrow_rate(params).unwrap();

            // Rate at target should remain unchanged when utilization is at target
            assert_eq_float!(
                curve.rate_at_target.to_float(),
                INITIAL_RATE_AT_TARGET.0.to_float()
            );
        }
    }

    #[test]
    fn test_rate_after_3_weeks_utilisation_target_ping_every_minute() {
        let mut curve = AdaptiveInterestRateCurve::new();

        // First establish the curve at target utilization
        let params = MarketBorrowRateParameters {
            utilisation_rate: &TARGET_UTILISATION_RATE,
            elapsed_seconds_since_last_update: days_to_seconds(1),
        };
        let rate = curve.borrow_rate(params).unwrap();
        assert_eq_float!(rate.to_float(), INITIAL_RATE_AT_TARGET.0.to_float());
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float()
        );

        let mut total_supply_assets = IFixedPoint::one();
        let mut total_borrow_assets = TARGET_UTILISATION_RATE;

        // Simulate 3 weeks of minute-by-minute updates
        let minutes_in_3_weeks = 3 * 7 * 24 * 60;
        for _ in 0..minutes_in_3_weeks {
            let utilisation_rate = total_borrow_assets.safe_div(total_supply_assets).unwrap();
            let params = MarketBorrowRateParameters {
                utilisation_rate: &utilisation_rate,
                elapsed_seconds_since_last_update: 60, // 1 minute
            };

            let avg_borrow_rate = curve.borrow_rate(params).unwrap();

            // Calculate compound interest for 1 minute
            let interest_factor = avg_borrow_rate
                .0
                .safe_mul(IFixedPoint::from_i64_u64_ratio(60, 1))
                .unwrap();
            let interest = total_borrow_assets.safe_mul(interest_factor).unwrap();

            total_supply_assets = total_supply_assets.safe_add(interest).unwrap();
            total_borrow_assets = total_borrow_assets.safe_add(interest).unwrap();
        }

        // Check final utilization is approximately at target (90%)
        let final_utilization = total_borrow_assets.safe_div(total_supply_assets).unwrap();
        assert_eq_float!(
            final_utilization.to_float(),
            TARGET_UTILISATION_RATE.to_float(),
            0.01
        );

        // Rate should be greater than or equal to initial rate
        assert!(curve.rate_at_target.to_float() >= INITIAL_RATE_AT_TARGET.0.to_float());

        // The rate is tolerated to be +10% (relatively) because of the pings every minute
        assert_eq_float!(
            curve.rate_at_target.to_float(),
            INITIAL_RATE_AT_TARGET.0.to_float(),
            0.1 // 10% tolerance
        );
    }
}
