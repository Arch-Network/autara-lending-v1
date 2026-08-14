use arch_program::pubkey::Pubkey;
use bytemuck::{Pod, Zeroable};

use crate::{
    error::{LendingError, LendingResult},
    padding::Padding,
};

crate::validate_struct!(LiquidatorWhitelistEntry, 80, 1);

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct LiquidatorWhitelistEntry {
    market: Pubkey,
    liquidator: Pubkey,
    bump: [u8; 1],
    active: [u8; 1],
    padding: Padding<14>,
}

impl LiquidatorWhitelistEntry {
    pub fn initialize(&mut self, market: Pubkey, liquidator: Pubkey, bump: u8) -> LendingResult {
        self.market = market;
        self.liquidator = liquidator;
        self.bump = [bump];
        self.activate()
    }

    #[inline(always)]
    pub fn market(&self) -> &Pubkey {
        &self.market
    }

    #[inline(always)]
    pub fn liquidator(&self) -> &Pubkey {
        &self.liquidator
    }

    #[inline(always)]
    pub fn bump(&self) -> &[u8; 1] {
        &self.bump
    }

    #[inline(always)]
    pub fn is_active(&self) -> bool {
        self.active == [1]
    }

    pub fn activate(&mut self) -> LendingResult {
        if self.is_active() {
            return Err(LendingError::LiquidatorAlreadyWhitelisted.into());
        }
        self.active = [1];
        Ok(())
    }

    pub fn deactivate(&mut self) -> LendingResult {
        if !self.is_active() {
            return Err(LendingError::LiquidatorNotWhitelisted.into());
        }
        self.active = [0];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_entry_transitions_are_checked() {
        let market = Pubkey::new_unique();
        let liquidator = Pubkey::new_unique();
        let mut entry = LiquidatorWhitelistEntry::default();

        entry.initialize(market, liquidator, 7).unwrap();
        assert!(entry.is_active());
        assert_eq!(entry.market(), &market);
        assert_eq!(entry.liquidator(), &liquidator);
        assert_eq!(entry.bump(), &[7]);
        assert_eq!(
            entry.activate().unwrap_err(),
            LendingError::LiquidatorAlreadyWhitelisted
        );

        entry.deactivate().unwrap();
        assert!(!entry.is_active());
        assert_eq!(
            entry.deactivate().unwrap_err(),
            LendingError::LiquidatorNotWhitelisted
        );

        entry.activate().unwrap();
        assert!(entry.is_active());
    }
}
