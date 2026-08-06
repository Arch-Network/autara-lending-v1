use arch_program::pubkey::Pubkey;
use borsh::{BorshDeserialize, BorshSerialize};
use bytemuck::{Pod, Zeroable};

use crate::{
    error::{LendingError, LendingResultExt},
    math::{ifixed_point::IFixedPoint, safe_math::SafeMath, ufixed_point::UFixedPoint},
    operation::liquidation::LiquidationResultWithBonus,
    padding::Padding,
};

use super::super::error::LendingResult;

crate::validate_struct!(BorrowPosition, 224);

#[repr(C)]
#[derive(Default, Clone, Copy, Pod, Zeroable, Debug)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct BorrowPosition {
    /// The authority which can manage this borrow position
    authority: Pubkey,
    /// The market this lending position is associated with
    market: Pubkey,
    /// Available collateral deposited in the position
    collateral_deposited_atoms: u64,
    /// Initial amount borrowed in atoms, will decrease proportionaly as the position is repaid
    initial_borrowed_atoms: u64,
    /// Track the total borrow shares of supply vault owned by this position
    borrowed_shares: UFixedPoint,
    pad: Padding<128>,
}

impl BorrowPosition {
    pub fn initialize(&mut self, authority: Pubkey, market: Pubkey) {
        self.authority = authority;
        self.market = market;
        self.collateral_deposited_atoms = 0;
        self.initial_borrowed_atoms = 0;
        self.borrowed_shares = UFixedPoint::zero();
    }

    #[inline(always)]
    pub fn authority(&self) -> &Pubkey {
        &self.authority
    }

    #[inline(always)]
    pub fn market(&self) -> &Pubkey {
        &self.market
    }

    #[inline(always)]
    pub fn borrowed_shares(&self) -> UFixedPoint {
        self.borrowed_shares
    }

    #[inline(always)]
    pub fn collateral_deposited_atoms(&self) -> u64 {
        self.collateral_deposited_atoms
    }

    #[inline(always)]
    pub fn initial_borrowed_atoms(&self) -> u64 {
        self.initial_borrowed_atoms
    }

    pub fn deposit_collateral(&mut self, atoms: u64) -> LendingResult {
        self.collateral_deposited_atoms = self.collateral_deposited_atoms.safe_add(atoms)?;
        Ok(())
    }

    pub fn withdraw_collateral(&mut self, atoms: u64) -> LendingResult {
        self.collateral_deposited_atoms = self
            .collateral_deposited_atoms
            .safe_sub(atoms)
            .map_err(|_| LendingError::WithdrawalExceedsDeposited)?;
        Ok(())
    }

    pub fn borrow(&mut self, atoms: u64, shares: UFixedPoint) -> LendingResult {
        self.initial_borrowed_atoms = self.initial_borrowed_atoms.safe_add(atoms)?;
        self.borrowed_shares = self.borrowed_shares.safe_add(shares)?;
        Ok(())
    }

    pub fn repay(&mut self, shares: UFixedPoint) -> LendingResult {
        let adjusted_atoms = match shares.cmp(&self.borrowed_shares) {
            std::cmp::Ordering::Less => shares
                .safe_div(self.borrowed_shares)?
                .safe_mul(self.initial_borrowed_atoms)?
                .as_u64_rounded_down()?,
            std::cmp::Ordering::Equal => {
                self.repay_all();
                return Ok(());
            }
            std::cmp::Ordering::Greater => {
                return Err(LendingError::RepayExceedsBorrowed.into());
            }
        };
        self.initial_borrowed_atoms = self.initial_borrowed_atoms.safe_sub(adjusted_atoms)?;
        self.borrowed_shares = self.borrowed_shares.safe_sub(shares)?;
        Ok(())
    }

    pub fn repay_all(&mut self) {
        self.initial_borrowed_atoms = 0;
        self.borrowed_shares = UFixedPoint::zero();
    }

    pub fn liquidate(
        &mut self,
        shares_liquidated: UFixedPoint,
        collateral_atoms_liquidated: u64,
    ) -> LendingResult {
        self.repay(shares_liquidated).track_caller()?;
        self.collateral_deposited_atoms = self
            .collateral_deposited_atoms
            .safe_sub(collateral_atoms_liquidated)?;
        Ok(())
    }
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct BorrowPositionHealth {
    pub ltv: IFixedPoint,
    pub borrowed_atoms: u64,
    pub collateral_atoms: u64,
    pub borrow_value: IFixedPoint,
    pub collateral_value: IFixedPoint,
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct LiquidationResultWithCtx {
    pub liquidation_result_with_bonus: LiquidationResultWithBonus,
    pub health_before_liquidation: BorrowPositionHealth,
    pub health_after_liquidation: BorrowPositionHealth,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_program::pubkey::Pubkey;

    fn create_position() -> BorrowPosition {
        let mut pos = BorrowPosition::default();
        pos.initialize(Pubkey::new_unique(), Pubkey::new_unique());
        pos
    }

    #[test]
    fn collateral_deposit_withdraw_roundtrip() {
        let mut pos = create_position();
        pos.deposit_collateral(1_000_000).unwrap();
        pos.withdraw_collateral(1_000_000).unwrap();
        assert_eq!(pos.collateral_deposited_atoms(), 0);
    }

    #[test]
    fn collateral_never_negative() {
        let mut pos = create_position();
        pos.deposit_collateral(1000).unwrap();
        let result = pos.withdraw_collateral(1001);
        assert!(result.is_err());
        assert_eq!(pos.collateral_deposited_atoms(), 1000);
    }

    #[test]
    fn borrow_increases_shares_and_atoms() {
        let mut pos = create_position();
        let shares = UFixedPoint::from_u64(1000);
        pos.borrow(1000, shares).unwrap();
        assert_eq!(pos.initial_borrowed_atoms(), 1000);
        assert_eq!(pos.borrowed_shares(), shares);
    }

    #[test]
    fn repay_all_clears_position() {
        let mut pos = create_position();
        pos.borrow(1000, UFixedPoint::from_u64(1000)).unwrap();
        pos.repay_all();
        assert_eq!(pos.initial_borrowed_atoms(), 0);
        assert!(pos.borrowed_shares().is_zero());
    }

    #[test]
    fn cant_repay_more_than_borrowed() {
        let mut pos = create_position();
        pos.borrow(1000, UFixedPoint::from_u64(1000)).unwrap();
        let result = pos.repay(UFixedPoint::from_u64(1001));
        assert!(result.is_err());
    }

    #[test]
    fn partial_repay_reduces_proportionally() {
        let mut pos = create_position();
        let initial_atoms = 1000;
        let initial_shares = UFixedPoint::from_u64(1000);
        pos.borrow(initial_atoms, initial_shares).unwrap();
        let half_shares = UFixedPoint::from_u64(500);
        pos.repay(half_shares).unwrap();
        assert!(pos.initial_borrowed_atoms() < initial_atoms);
        assert!(pos.borrowed_shares() < initial_shares);
    }

    #[test]
    fn liquidate_reduces_both_debt_and_collateral() {
        let mut pos = create_position();
        pos.deposit_collateral(10000).unwrap();
        pos.borrow(5000, UFixedPoint::from_u64(5000)).unwrap();
        let collateral_before = pos.collateral_deposited_atoms();
        let shares_before = pos.borrowed_shares();
        pos.liquidate(UFixedPoint::from_u64(1000), 2000).unwrap();
        assert!(pos.collateral_deposited_atoms() < collateral_before);
        assert!(pos.borrowed_shares() < shares_before);
    }

    #[test]
    fn multiple_deposits_accumulate() {
        let mut pos = create_position();
        pos.deposit_collateral(1000).unwrap();
        pos.deposit_collateral(2000).unwrap();
        pos.deposit_collateral(3000).unwrap();
        assert_eq!(pos.collateral_deposited_atoms(), 6000);
    }

    #[test]
    fn multiple_borrows_accumulate() {
        let mut pos = create_position();
        pos.borrow(1000, UFixedPoint::from_u64(1000)).unwrap();
        pos.borrow(2000, UFixedPoint::from_u64(2000)).unwrap();
        assert_eq!(pos.initial_borrowed_atoms(), 3000);
        assert_eq!(pos.borrowed_shares(), UFixedPoint::from_u64(3000));
    }

    #[test]
    fn initialize_resets_all_fields() {
        let mut pos = create_position();
        pos.deposit_collateral(1000).unwrap();
        pos.borrow(500, UFixedPoint::from_u64(500)).unwrap();
        let new_auth = Pubkey::new_unique();
        let new_market = Pubkey::new_unique();
        pos.initialize(new_auth, new_market);
        assert_eq!(pos.collateral_deposited_atoms(), 0);
        assert_eq!(pos.initial_borrowed_atoms(), 0);
        assert!(pos.borrowed_shares().is_zero());
        assert_eq!(pos.authority(), &new_auth);
        assert_eq!(pos.market(), &new_market);
    }
}
