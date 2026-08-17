use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    error::LendingResult,
    interest_rate::{
        curve::{adaptative_curve::AdaptiveInterestRateCurve, polyline::PolylineInterestRateCurve},
        interest_rate_per_second::InterestRatePerSecond,
        lending_interest_rate::{LendingInterestRateCurveMut, MarketBorrowRateParameters},
    },
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "type", content = "content")
)]
pub enum InterestRateCurveKind {
    Fixed(InterestRatePerSecond),
    Polyline(PolylineInterestRateCurve),
    Adaptive(AdaptiveInterestRateCurve),
}

impl InterestRateCurveKind {
    pub fn new_approximate_fixed_apy(apy: f64) -> Self {
        InterestRateCurveKind::Fixed(InterestRatePerSecond::approximate_from_apy(apy))
    }

    pub fn new_adaptive() -> Self {
        InterestRateCurveKind::Adaptive(AdaptiveInterestRateCurve::new())
    }

    pub fn is_valid(&self) -> bool {
        match self {
            InterestRateCurveKind::Fixed(rate) => {
                // Reject negative rates (would cause NegativeInterestRate in sync_clock)
                // and rates that would overflow checked_exp after 1 second (MAX_EXP_ARG ≈ 54.75)
                !rate.0.is_negative()
                    && rate.0 <= crate::math::ifixed_point::IFixedPoint::from_i64_u64_ratio(5475, 100)
            }
            InterestRateCurveKind::Polyline(curve) => curve.validate().is_ok(),
            InterestRateCurveKind::Adaptive(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::ifixed_point::IFixedPoint;

    #[test]
    fn fixed_negative_rate_is_invalid() {
        let curve = InterestRateCurveKind::Fixed(InterestRatePerSecond::new(
            IFixedPoint::lit("-0.1"),
        ));
        assert!(!curve.is_valid());
    }

    #[test]
    fn fixed_rate_exceeding_max_exp_arg_is_invalid() {
        let curve = InterestRateCurveKind::Fixed(InterestRatePerSecond::new(
            IFixedPoint::from_i64(56),
        ));
        assert!(!curve.is_valid());
    }

    #[test]
    fn fixed_zero_rate_is_valid() {
        let curve = InterestRateCurveKind::Fixed(InterestRatePerSecond::new(IFixedPoint::zero()));
        assert!(curve.is_valid());
    }

    #[test]
    fn fixed_reasonable_rate_is_valid() {
        let curve = InterestRateCurveKind::new_approximate_fixed_apy(0.10);
        assert!(curve.is_valid());
    }
}

impl Default for InterestRateCurveKind {
    fn default() -> Self {
        InterestRateCurveKind::Fixed(InterestRatePerSecond::approximate_from_apy(0.1))
    }
}

impl LendingInterestRateCurveMut for InterestRateCurveKind {
    fn borrow_rate_per_second(
        &mut self,
        params: MarketBorrowRateParameters,
    ) -> LendingResult<InterestRatePerSecond> {
        match self {
            InterestRateCurveKind::Fixed(rate) => Ok(*rate),
            InterestRateCurveKind::Polyline(curve) => {
                Ok(curve.interest_rate_per_second(params.utilisation_rate_bps()?))
            }
            InterestRateCurveKind::Adaptive(curve) => curve.borrow_rate(params),
        }
    }
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum InterestRateKindCurveMut<'a> {
    Fixed(&'a mut InterestRatePerSecond),
    Polyline(&'a mut PolylineInterestRateCurve),
    Adaptive(&'a mut AdaptiveInterestRateCurve),
}

impl LendingInterestRateCurveMut for InterestRateKindCurveMut<'_> {
    fn borrow_rate_per_second(
        &mut self,
        params: MarketBorrowRateParameters,
    ) -> LendingResult<InterestRatePerSecond> {
        match self {
            InterestRateKindCurveMut::Fixed(rate) => Ok(**rate),
            InterestRateKindCurveMut::Polyline(curve) => {
                Ok(curve.interest_rate_per_second(params.utilisation_rate_bps()?))
            }
            InterestRateKindCurveMut::Adaptive(curve) => curve.borrow_rate(params),
        }
    }
}
