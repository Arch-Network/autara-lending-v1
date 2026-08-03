use arch_program::pubkey::Pubkey;
use borsh::{BorshDeserialize, BorshSerialize};
use bytemuck::{Pod, Zeroable};

use crate::{
    error::{LendingError, LendingResult, LendingResultExt},
    math::{
        bps::{bps_to_fixed_point, percent_to_bps},
        ifixed_point::IFixedPoint,
        safe_math::SafeMath,
        ufixed_point::UFixedPoint,
    },
    padding::Padding,
    state::global_config::GlobalConfig,
};

crate::validate_struct!(MarketConfig, 192);

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct MarketConfig {
    bump: [u8; 1],
    index: [u8; 1],
    pad_1: Padding<2>,
    /// Total fee in bps which is charged on lending
    lending_market_fee_in_bps: u16,
    /// Share of the fee which is sent to the protocol
    protocol_fee_share_in_bps: u16,
    /// Curator of the market, can update the market config
    curator: Pubkey,
    /// Loan to value configuration of the market
    ltv_config: LtvConfig,
    /// Max utilisation rate of the supply vault after a borrow
    /// Withdrawals and deposits wont be affected by this constraint
    max_utilisation_rate: IFixedPoint,
    /// Maximum atoms which can be supplied to the market
    max_supply_atoms: u64,
    /// Number of active liquidator whitelist entries. Zero keeps liquidation permissionless.
    active_whitelisted_liquidators: u64,
    pad_2: Padding<72>,
}

pub const MAX_LTV_WITH_LIQUIDATION_BONUS: IFixedPoint = IFixedPoint::lit("0.99");
pub const MAX_LIQUIDATION_BONUS: IFixedPoint = IFixedPoint::lit("0.1");
pub const MIN_LIQUIDATION_BONUS: IFixedPoint = IFixedPoint::lit("0.001");
pub const MAX_UTILISATION_RATE: IFixedPoint = IFixedPoint::lit("0.99");
pub const MAX_LENDING_MARKET_FEE_IN_BPS: u16 = percent_to_bps(20) as u16;
pub const FEE_PERCENT_FOR_PROTOCOL_IN_BPS: u16 = percent_to_bps(50) as u16;

pub const TARGET_LTV_LIQUIDATION_MARGIN: IFixedPoint = IFixedPoint::lit("0.90");

impl MarketConfig {
    #[inline(always)]
    pub fn bump(&self) -> &[u8; 1] {
        &self.bump
    }

    #[inline(always)]
    pub fn index(&self) -> &[u8; 1] {
        &self.index
    }

    #[inline(always)]
    pub fn lending_market_fee_in_bps(&self) -> u16 {
        self.lending_market_fee_in_bps
    }

    #[inline(always)]
    pub fn protocol_fee_in_bps(&self) -> u16 {
        self.protocol_fee_share_in_bps
    }

    #[inline(always)]
    pub fn curator(&self) -> &Pubkey {
        &self.curator
    }

    #[inline(always)]
    pub fn ltv_config(&self) -> &LtvConfig {
        &self.ltv_config
    }

    #[inline(always)]
    pub fn max_utilisation_rate(&self) -> IFixedPoint {
        self.max_utilisation_rate
    }

    #[inline(always)]
    pub fn max_supply_atoms(&self) -> u64 {
        self.max_supply_atoms
    }

    #[inline(always)]
    pub fn active_whitelisted_liquidators(&self) -> u64 {
        self.active_whitelisted_liquidators
    }

    #[inline(always)]
    pub fn liquidations_are_permissionless(&self) -> bool {
        self.active_whitelisted_liquidators == 0
    }

    pub fn increment_active_whitelisted_liquidators(&mut self) -> LendingResult {
        self.active_whitelisted_liquidators = self.active_whitelisted_liquidators.safe_add(1)?;
        Ok(())
    }

    pub fn decrement_active_whitelisted_liquidators(&mut self) -> LendingResult {
        self.active_whitelisted_liquidators = self.active_whitelisted_liquidators.safe_sub(1)?;
        Ok(())
    }

    #[inline(always)]
    pub fn lending_market_fee_fixed(&self) -> UFixedPoint {
        bps_to_fixed_point(self.lending_market_fee_in_bps() as u64)
    }

    pub fn initialize(
        &mut self,
        bump: u8,
        index: u8,
        curator: &Pubkey,
        ltv_config: &LtvConfig,
        max_utilisation_rate: IFixedPoint,
        max_supply_atoms: u64,
        lending_market_fee_in_bps: u16,
        global_config: &GlobalConfig,
    ) -> LendingResult {
        self.update_ltv(ltv_config)?;
        self.update_max_utilisation_rate(max_utilisation_rate)?;
        self.set_lending_market_fee(lending_market_fee_in_bps)?;
        self.bump = [bump];
        self.index = [index];
        self.curator = *curator;
        self.max_supply_atoms = max_supply_atoms;
        self.sync_global_config(global_config);
        Ok(())
    }

    pub fn sync_global_config(&mut self, global_config: &GlobalConfig) {
        self.protocol_fee_share_in_bps = global_config.protocol_fee_share_in_bps();
    }

    pub fn update_max_supply_atoms(&mut self, max_supply_atoms: u64) {
        self.max_supply_atoms = max_supply_atoms;
    }

    pub fn set_lending_market_fee(&mut self, lending_market_fee_in_bps: u16) -> LendingResult {
        if lending_market_fee_in_bps > MAX_LENDING_MARKET_FEE_IN_BPS {
            return Err(LendingError::FeeTooHigh.into());
        }
        self.lending_market_fee_in_bps = lending_market_fee_in_bps;
        Ok(())
    }

    pub fn update_max_utilisation_rate(
        &mut self,
        max_utilisation_rate: IFixedPoint,
    ) -> LendingResult {
        if max_utilisation_rate > MAX_UTILISATION_RATE {
            return Err(LendingError::InvalidMaxUtilisationRate.into());
        }
        self.max_utilisation_rate = max_utilisation_rate;
        Ok(())
    }

    pub fn update_ltv(&mut self, ltv_config: &LtvConfig) -> LendingResult {
        if ltv_config.max_ltv.is_negative()
            || ltv_config.unhealthy_ltv.is_negative()
            || ltv_config.liquidation_bonus.is_negative()
        {
            return Err(LendingError::InvalidLtvConfig.into())
                .with_msg("ltv values must be non-negative");
        }
        // cant reduce the unhealthy ltv, it could make some positions immediately unhealthy
        if self.ltv_config.unhealthy_ltv > ltv_config.unhealthy_ltv {
            return Err(LendingError::InvalidLtvConfig.into())
                .with_msg("cannot reduce unhealthy ltv");
        }
        // cant have max ltv greater than unhealthy ltv
        if ltv_config.max_ltv >= ltv_config.unhealthy_ltv {
            return Err(LendingError::InvalidLtvConfig.into())
                .with_msg("max ltv must be less than unhealthy ltv");
        }
        // cant have liquidation bonus greater than 10% or lower than 0.5%
        if ltv_config.liquidation_bonus > MAX_LIQUIDATION_BONUS
            || ltv_config.liquidation_bonus < MIN_LIQUIDATION_BONUS
        {
            return Err(LendingError::InvalidLtvConfig.into())
                .with_msg("liquidation bonus out of range");
        }
        let one_plus_liquidation_bonus =
            IFixedPoint::one().safe_add(ltv_config.liquidation_bonus)?;
        let unhealthy_ltv_with_liquidation_bonus = ltv_config
            .unhealthy_ltv
            .safe_mul(one_plus_liquidation_bonus)?;
        // we must ensure that there is enough margin for liquidation bonus
        if unhealthy_ltv_with_liquidation_bonus > MAX_LTV_WITH_LIQUIDATION_BONUS {
            return Err(LendingError::InvalidLtvConfig.into())
                .with_msg("insufficient margin for liquidation bonus");
        }
        self.ltv_config = *ltv_config;
        Ok(())
    }
}

/// Loan to value configuration of the market
/// Can be updated by the curator
#[repr(C)]
#[derive(
    Debug, Clone, Copy, Pod, Zeroable, BorshSerialize, BorshDeserialize, PartialEq, Eq, Default,
)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct LtvConfig {
    /// Maximum loan to value ratio which can be opened
    /// Cannot be greater than unhealthy_ltv
    /// Can be updated by the curator
    pub max_ltv: IFixedPoint,
    /// LTV from which the position is considered unhealthy and can be liquidated
    /// Cannot be lower than max_ltv
    /// Can only be increased by the curator
    pub unhealthy_ltv: IFixedPoint,
    /// Bonus for liquidating a position
    /// Cannot be greater than 10% or lower than 0.5%
    /// Can be updated by the curator
    /// Cannot be set so high that there is not enough margin for the liquidation bonus
    /// i.e. unhealthy_ltv * (1 + liquidation_bonus) <= 99%
    pub liquidation_bonus: IFixedPoint,
}

impl LtvConfig {
    pub fn target_ltv_after_liquidation(&self) -> IFixedPoint {
        self.unhealthy_ltv
            .safe_mul(TARGET_LTV_LIQUIDATION_MARGIN)
            .unwrap_or_else(|_| self.unhealthy_ltv) // should never happen
            .max(self.max_ltv)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::{math::bps::percent_to_bps, state::global_config::tests::test_global_config};

    pub fn test_config() -> MarketConfig {
        MarketConfig {
            bump: [0; 1],
            index: [0; 1],
            pad_1: Padding::default(),
            curator: Pubkey::new_unique(),
            ltv_config: LtvConfig {
                max_ltv: IFixedPoint::from(0.8),
                unhealthy_ltv: IFixedPoint::from(0.9),
                liquidation_bonus: IFixedPoint::from(0.05),
            },
            max_utilisation_rate: IFixedPoint::from(0.95),
            lending_market_fee_in_bps: percent_to_bps(10) as u16,
            protocol_fee_share_in_bps: percent_to_bps(50) as u16,
            max_supply_atoms: u64::MAX,
            active_whitelisted_liquidators: 0,
            pad_2: Padding::default(),
        }
    }

    #[test]
    fn whitelist_count_controls_permissionless_mode() {
        let mut config = test_config();
        assert!(config.liquidations_are_permissionless());

        config.increment_active_whitelisted_liquidators().unwrap();
        assert_eq!(config.active_whitelisted_liquidators(), 1);
        assert!(!config.liquidations_are_permissionless());

        config.decrement_active_whitelisted_liquidators().unwrap();
        assert_eq!(config.active_whitelisted_liquidators(), 0);
        assert!(config.liquidations_are_permissionless());
    }

    #[test]
    fn whitelist_count_rejects_overflow_and_underflow_without_mutating() {
        let mut config = test_config();
        assert_eq!(
            config
                .decrement_active_whitelisted_liquidators()
                .unwrap_err(),
            LendingError::SubtractionOverflow
        );
        assert_eq!(config.active_whitelisted_liquidators(), 0);

        config.active_whitelisted_liquidators = u64::MAX;
        assert_eq!(
            config
                .increment_active_whitelisted_liquidators()
                .unwrap_err(),
            LendingError::AdditionOverflow
        );
        assert_eq!(config.active_whitelisted_liquidators(), u64::MAX);
    }

    #[test]
    fn test_initialization() {
        let mut market_config = MarketConfig::default();
        let curator = Pubkey::new_unique();
        let ltv_config = LtvConfig {
            max_ltv: IFixedPoint::from(0.8),
            unhealthy_ltv: IFixedPoint::from(0.9),
            liquidation_bonus: IFixedPoint::from(0.05),
        };
        let max_utilisation_rate = IFixedPoint::from(0.95);
        let max_supply_atoms = 1_000_000;
        let lending_market_fee_in_bps = percent_to_bps(10) as u16;
        let global_config = test_global_config();

        let result = market_config.initialize(
            1,
            0,
            &curator,
            &ltv_config,
            max_utilisation_rate,
            max_supply_atoms,
            lending_market_fee_in_bps,
            &global_config,
        );

        assert!(result.is_ok());
        assert_eq!(market_config.bump(), &[1]);
        assert_eq!(market_config.curator(), &curator);
        assert_eq!(market_config.ltv_config(), &ltv_config);
        assert_eq!(market_config.max_utilisation_rate(), max_utilisation_rate);
        assert_eq!(market_config.max_supply_atoms(), max_supply_atoms);
        assert_eq!(
            market_config.lending_market_fee_in_bps(),
            lending_market_fee_in_bps
        );
        assert_eq!(
            market_config.protocol_fee_in_bps(),
            global_config.protocol_fee_share_in_bps()
        );
    }

    #[test]
    fn test_set_lending_market_fee() {
        let mut market_config = test_config();

        // Valid fee
        let valid_fee = percent_to_bps(15) as u16;
        let result = market_config.set_lending_market_fee(valid_fee);
        assert!(result.is_ok());
        assert_eq!(market_config.lending_market_fee_in_bps(), valid_fee);

        // Fee too high
        let invalid_fee = MAX_LENDING_MARKET_FEE_IN_BPS + 1;
        let result = market_config.set_lending_market_fee(invalid_fee);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::FeeTooHigh
        ));
    }

    #[test]
    fn test_update_max_utilisation_rate() {
        let mut market_config = test_config();

        // Valid utilization rate
        let valid_rate = IFixedPoint::from(0.9);
        let result = market_config.update_max_utilisation_rate(valid_rate);
        assert!(result.is_ok());
        assert_eq!(market_config.max_utilisation_rate(), valid_rate);

        // Rate too high
        let invalid_rate = MAX_UTILISATION_RATE
            .safe_add(IFixedPoint::from(0.01))
            .unwrap();
        let result = market_config.update_max_utilisation_rate(invalid_rate);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidMaxUtilisationRate
        ));
    }

    #[test]
    fn test_update_ltv() {
        let mut market_config = test_config();

        // Valid LTV config
        let valid_ltv_config = LtvConfig {
            max_ltv: IFixedPoint::from(0.7),
            unhealthy_ltv: IFixedPoint::from(0.9), // Same as before
            liquidation_bonus: IFixedPoint::from(0.08),
        };
        let result = market_config.update_ltv(&valid_ltv_config);
        assert!(result.is_ok());
        assert_eq!(market_config.ltv_config(), &valid_ltv_config);

        // Invalid - reducing unhealthy LTV
        let invalid_ltv_config = LtvConfig {
            max_ltv: IFixedPoint::from(0.7),
            unhealthy_ltv: IFixedPoint::from(0.8), // Lower than original
            liquidation_bonus: IFixedPoint::from(0.05),
        };
        let result = market_config.update_ltv(&invalid_ltv_config);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));

        // Invalid - max LTV > unhealthy LTV
        let invalid_ltv_config = LtvConfig {
            max_ltv: IFixedPoint::from(0.95),
            unhealthy_ltv: IFixedPoint::from(0.9),
            liquidation_bonus: IFixedPoint::from(0.05),
        };
        let result = market_config.update_ltv(&invalid_ltv_config);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));

        // Invalid - liquidation bonus too high
        let invalid_ltv_config = LtvConfig {
            max_ltv: IFixedPoint::from(0.7),
            unhealthy_ltv: IFixedPoint::from(0.9),
            liquidation_bonus: IFixedPoint::from(0.15), // > MAX_LIQUIDATION_BONUS
        };
        let result = market_config.update_ltv(&invalid_ltv_config);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));

        // Invalid - not enough margin for liquidation bonus
        let invalid_ltv_config = LtvConfig {
            max_ltv: IFixedPoint::from(0.85),
            unhealthy_ltv: IFixedPoint::from(0.95),
            liquidation_bonus: IFixedPoint::from(0.09),
        };
        let result = market_config.update_ltv(&invalid_ltv_config);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));
    }

    #[test]
    fn test_update_ltv_rejects_negative_max_ltv() {
        let mut market_config = test_config();
        let invalid_ltv = LtvConfig {
            max_ltv: IFixedPoint::from_i64_u64_ratio(-1, 100),
            unhealthy_ltv: IFixedPoint::from(0.9),
            liquidation_bonus: IFixedPoint::from(0.05),
        };
        let result = market_config.update_ltv(&invalid_ltv);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));
    }

    #[test]
    fn test_update_ltv_rejects_negative_unhealthy_ltv() {
        let mut market_config = MarketConfig::default();
        let invalid_ltv = LtvConfig {
            max_ltv: IFixedPoint::from(0.5),
            unhealthy_ltv: IFixedPoint::from_i64_u64_ratio(-1, 100),
            liquidation_bonus: IFixedPoint::from(0.05),
        };
        let result = market_config.update_ltv(&invalid_ltv);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));
    }

    #[test]
    fn test_update_ltv_rejects_negative_liquidation_bonus() {
        let mut market_config = MarketConfig::default();
        let invalid_ltv = LtvConfig {
            max_ltv: IFixedPoint::from(0.5),
            unhealthy_ltv: IFixedPoint::from(0.9),
            liquidation_bonus: IFixedPoint::from_i64_u64_ratio(-1, 100),
        };
        let result = market_config.update_ltv(&invalid_ltv);
        assert!(matches!(
            result.unwrap_err().error,
            LendingError::InvalidLtvConfig
        ));
    }

    #[test]
    fn test_sync_global_config() {
        let mut market_config = test_config();
        let mut global_config = test_global_config();

        // Change the protocol fee share in global config
        let new_fee = percent_to_bps(15) as u16;
        global_config
            .update_protocol_fee_share_in_bps(new_fee)
            .unwrap();

        market_config.sync_global_config(&global_config);
        assert_eq!(market_config.protocol_fee_in_bps(), new_fee);
    }

    #[test]
    fn test_update_max_supply_atoms() {
        let mut market_config = test_config();
        let new_max_supply = 5_000_000;

        market_config.update_max_supply_atoms(new_max_supply);
        assert_eq!(market_config.max_supply_atoms(), new_max_supply);
    }

    #[test]
    fn test_accessors() {
        let config = test_config();

        assert_eq!(
            config.lending_market_fee_fixed(),
            bps_to_fixed_point(config.lending_market_fee_in_bps() as u64)
        );
    }
}
