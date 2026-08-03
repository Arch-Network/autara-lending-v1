use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::{borrow_position::BorrowPosition, market::Market};
use autara_program_lib::accounts::{
    packed::PackedOwnedAccount,
    program::Program,
    signer::Signer,
    token::{AplTokenProgram, TokenAccount},
    zero_copy::ZeroCopyOwnedAccountMut,
};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    state::AutaraAccount,
};

pub struct SettleCapitalSweepAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub borrow_position: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<BorrowPosition>>,
    pub curator: Signer<'a, 'b>,
    pub curator_supply_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub curator_collateral_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_collateral_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub supply_oracle: &'b AccountInfo<'a>,
    pub collateral_oracle: &'b AccountInfo<'a>,
}

impl<'a, 'b> SettleCapitalSweepAccounts<'a, 'b> {
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
            borrow_position: next_account_info(accounts)?.try_into()?,
            curator: next_account_info(accounts)?.try_into()?,
            curator_supply_ata: next_account_info(accounts)?.try_into()?,
            curator_collateral_ata: next_account_info(accounts)?.try_into()?,
            market_supply_vault: next_account_info(accounts)?.try_into()?,
            market_collateral_vault: next_account_info(accounts)?.try_into()?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
            supply_oracle: next_account_info(accounts)?,
            collateral_oracle: next_account_info(accounts)?,
        };
        this.validate()?;
        Ok(this)
    }

    fn validate(&self) -> LendingProgramResult {
        let borrow_position = self.borrow_position.load_ref();
        let market = self.market.load_ref();
        if borrow_position.market() != self.market.key() {
            return Err(LendingAccountValidationError::InvalidMarket.into());
        }
        if market.config().curator() != self.curator.key {
            return Err(LendingAccountValidationError::InvalidMarketAuthority.into());
        }
        if &self.curator_supply_ata.mint != market.supply_vault().mint()
            || &self.market_supply_vault.mint != market.supply_vault().mint()
        {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        if &self.curator_collateral_ata.mint != market.collateral_vault().mint()
            || &self.market_collateral_vault.mint != market.collateral_vault().mint()
        {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        if market.supply_vault().vault() != self.market_supply_vault.key()
            || market.collateral_vault().vault() != self.market_collateral_vault.key()
        {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use autara_program_lib::accounts::AccountValidationError;

    use super::*;
    use crate::{error::LendingAccountValidationError, ixs::test_utils::AutaraAccounts};

    fn settle_accounts(
        accounts: &AutaraAccounts,
    ) -> Vec<arch_program::account::AccountInfo<'static>> {
        vec![
            accounts.market.clone(),
            accounts.borrow_position.clone(),
            accounts.curator.clone(),
            accounts.user_supply_ata.clone(),
            accounts.user_collateral_ata.clone(),
            accounts.market_supply_vault.clone(),
            accounts.market_collateral_vault.clone(),
            accounts.apl_token_program.clone(),
            accounts.oracle.clone(),
            accounts.oracle.clone(),
        ]
    }

    #[test]
    fn validates_settle_capital_sweep_accounts() {
        let accounts = AutaraAccounts::new();
        let account_infos = settle_accounts(&accounts);

        SettleCapitalSweepAccounts::from_accounts(&mut account_infos.iter()).unwrap();
    }

    #[test]
    fn rejects_non_signing_or_wrong_curator_for_settlement() {
        let mut accounts = AutaraAccounts::new();
        accounts.curator.non_signer();
        let account_infos = settle_accounts(&accounts);
        assert_eq!(
            SettleCapitalSweepAccounts::from_accounts(&mut account_infos.iter())
                .err()
                .unwrap(),
            AccountValidationError::NotSigner
        );

        let market_accounts = AutaraAccounts::new();
        let wrong_curator_accounts = AutaraAccounts::new();
        let mut account_infos = settle_accounts(&market_accounts);
        account_infos[2] = wrong_curator_accounts.curator.clone();
        assert_eq!(
            SettleCapitalSweepAccounts::from_accounts(&mut account_infos.iter())
                .err()
                .unwrap(),
            LendingAccountValidationError::InvalidMarketAuthority
        );
    }

    #[test]
    fn rejects_wrong_supply_or_collateral_accounts_for_settlement() {
        let market_accounts = AutaraAccounts::new();
        let other_accounts = AutaraAccounts::new();

        let mut wrong_supply = settle_accounts(&market_accounts);
        wrong_supply[3] = other_accounts.user_collateral_ata.clone();
        assert_eq!(
            SettleCapitalSweepAccounts::from_accounts(&mut wrong_supply.iter())
                .err()
                .unwrap(),
            LendingAccountValidationError::InvalidMintForTokenAccount
        );

        let mut wrong_collateral_vault = settle_accounts(&market_accounts);
        wrong_collateral_vault[6] = market_accounts.user_collateral_ata.clone();
        assert_eq!(
            SettleCapitalSweepAccounts::from_accounts(&mut wrong_collateral_vault.iter())
                .err()
                .unwrap(),
            LendingAccountValidationError::InvalidMarketVault
        );
    }
}
