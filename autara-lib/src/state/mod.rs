pub mod borrow_position;
pub mod collateral_vault;
pub mod global_config;
pub mod liquidator_whitelist;
pub mod market;
pub mod market_config;
pub mod market_wrapper;
pub mod supply_position;
pub mod supply_vault;

// Autara Lending Accounts are discriminated by their size.
const _: () = const {
    let accounts_size = [
        size_of::<borrow_position::BorrowPosition>(),
        size_of::<supply_position::SupplyPosition>(),
        size_of::<market::Market>(),
        size_of::<global_config::GlobalConfig>(),
        size_of::<liquidator_whitelist::LiquidatorWhitelistEntry>(),
    ];
    validate_all_different_sizes(accounts_size);
};

#[allow(dead_code)]
const fn validate_all_different_sizes<const M: usize>(sizes: [usize; M]) {
    let mut i = 0;
    while i < sizes.len() {
        let mut j = i + 1;
        while j < sizes.len() {
            if sizes[i] == sizes[j] {
                panic!("duplicated size");
            }
            j += 1;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        constant::SECONDS_PER_YEAR,
        error::LendingError,
        interest_rate::interest_rate_kind::InterestRateCurveKind,
        oracle::oracle_config::tests::{default_btc_oracle_rate, default_usd_oracle_rate},
        state::{
            borrow_position::BorrowPosition, collateral_vault::tests::BTC,
            market::tests::create_empty_btc_usdc_market, supply_position::SupplyPosition,
            supply_vault::tests::USDC,
        },
    };

    use super::*;

    #[test]
    fn test_validate_all_different_sizes() {
        validate_all_different_sizes([1, 2, 3, 4]);
    }

    #[test]
    #[should_panic(expected = "duplicated size")]
    fn test_validate_all_different_sizes_fail() {
        validate_all_different_sizes([2, 2, 4, 3]);
    }

    #[test]
    fn poc_adaptive_curve_can_perma_brick_market_after_long_idle() {
        // Setup a market using the adaptive curve and allow 100% utilisation for the test.
        let mut market = create_empty_btc_usdc_market();
        market
            .config_mut()
            .update_max_utilisation_rate(crate::math::ifixed_point::IFixedPoint::lit("0.99"))
            .unwrap();

        // Re-initialize the supply vault to use the adaptive curve (test helpers default to fixed).
        let supply_mint = *market.supply_vault().mint();
        let supply_decimals = market.supply_vault().mint_decimals() as u64;
        let supply_vault = *market.supply_vault().vault();
        let supply_oracle_config = *market.supply_vault().oracle_config();
        market
            .initlize_supply_vault(
                supply_mint,
                supply_decimals,
                supply_vault,
                supply_oracle_config,
                InterestRateCurveKind::new_adaptive(),
                0, // last_update_unix_timestamp
            )
            .unwrap();

        // First sync initializes the adaptive curve state (`rate_at_target` becomes non-zero).
        market.sync_clock(1).unwrap();

        // Make utilisation > target (90%): lend then borrow all supplied atoms.
        let lend_atoms = USDC(1_000.0);
        let mut supply_position = SupplyPosition::default();
        market.lend(&mut supply_position, lend_atoms).unwrap();

        let mut borrow_position = BorrowPosition::default();
        market
            .deposit_collateral(&mut borrow_position, BTC(1.0))
            .unwrap();
        let supply_oracle = default_usd_oracle_rate();
        let collateral_oracle = default_btc_oracle_rate();
        let borrow_atoms = (lend_atoms as f64 * 0.99) as u64; // 99% utilisation (at cap)
        market
            .borrow(
                &mut borrow_position,
                borrow_atoms,
                &supply_oracle,
                &collateral_oracle,
            )
            .unwrap();

        let last_update_before = market
            .supply_vault()
            .get_summary()
            .unwrap()
            .last_update_unix_timestamp;
        assert_eq!(last_update_before, 1);

        // Simulate a long period with no successful sync, then attempt to sync again at ~2y later.
        // With utilisation = 100%, err = 1 and linear_adaptation ≈ 50 * years_elapsed.
        // After ~1.1y, linear_adaptation > 55.26 and `checked_exp()` errors.
        let brick_timestamp = last_update_before + (2 * SECONDS_PER_YEAR) as i64;
        assert_eq!(
            market.sync_clock(brick_timestamp).unwrap_err(),
            LendingError::InvalidExpArg
        );

        // `last_update_unix_timestamp` is not advanced on error → market remains stuck.
        let last_update_after = market
            .supply_vault()
            .get_summary()
            .unwrap()
            .last_update_unix_timestamp;
        assert_eq!(last_update_after, last_update_before);

        // Any later attempt still fails since elapsed grows while `last_update_unix_timestamp` stays fixed.
        assert_eq!(
            market
                .sync_clock(brick_timestamp + SECONDS_PER_YEAR as i64)
                .unwrap_err(),
            LendingError::InvalidExpArg
        );
    }
}
