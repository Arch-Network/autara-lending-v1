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

pub struct BeginCapitalSweepAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub borrow_position: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<BorrowPosition>>,
    pub curator: Signer<'a, 'b>,
    pub curator_collateral_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_collateral_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub supply_oracle: &'b AccountInfo<'a>,
    pub collateral_oracle: &'b AccountInfo<'a>,
}

impl<'a, 'b> BeginCapitalSweepAccounts<'a, 'b> {
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
            curator_collateral_ata: next_account_info(accounts)?.try_into()?,
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
        if &self.curator_collateral_ata.mint != market.collateral_vault().mint() {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        if market.collateral_vault().vault() != self.market_collateral_vault.key() {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        if &self.market_collateral_vault.mint != market.collateral_vault().mint() {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use autara_program_lib::accounts::AccountValidationError;

    use super::*;
    use crate::{error::LendingAccountValidationError, ixs::test_utils::AutaraAccounts};

    #[test]
    fn validates_begin_capital_sweep_accounts() {
        let accounts = AutaraAccounts::new();
        let account_infos = [
            accounts.market.clone(),
            accounts.borrow_position.clone(),
            accounts.curator.clone(),
            accounts.user_collateral_ata.clone(),
            accounts.market_collateral_vault.clone(),
            accounts.apl_token_program.clone(),
            accounts.oracle.clone(),
            accounts.oracle.clone(),
        ];

        BeginCapitalSweepAccounts::from_accounts(&mut account_infos.iter()).unwrap();
    }

    #[test]
    fn rejects_non_signing_or_wrong_curator_for_begin() {
        let mut non_signing = AutaraAccounts::new();
        non_signing.curator.non_signer();
        let account_infos = [
            non_signing.market.clone(),
            non_signing.borrow_position.clone(),
            non_signing.curator.clone(),
            non_signing.user_collateral_ata.clone(),
            non_signing.market_collateral_vault.clone(),
            non_signing.apl_token_program.clone(),
            non_signing.oracle.clone(),
            non_signing.oracle.clone(),
        ];
        assert_eq!(
            BeginCapitalSweepAccounts::from_accounts(&mut account_infos.iter())
                .err()
                .unwrap(),
            AccountValidationError::NotSigner
        );

        let market_accounts = AutaraAccounts::new();
        let wrong_curator_accounts = AutaraAccounts::new();
        let account_infos = [
            market_accounts.market.clone(),
            market_accounts.borrow_position.clone(),
            wrong_curator_accounts.curator.clone(),
            market_accounts.user_collateral_ata.clone(),
            market_accounts.market_collateral_vault.clone(),
            market_accounts.apl_token_program.clone(),
            market_accounts.oracle.clone(),
            market_accounts.oracle.clone(),
        ];
        assert_eq!(
            BeginCapitalSweepAccounts::from_accounts(&mut account_infos.iter())
                .err()
                .unwrap(),
            LendingAccountValidationError::InvalidMarketAuthority
        );
    }

    #[test]
    fn rejects_wrong_market_vault_or_position_for_begin() {
        let market_accounts = AutaraAccounts::new();
        let other_accounts = AutaraAccounts::new();
        let account_infos = [
            market_accounts.market.clone(),
            other_accounts.borrow_position.clone(),
            market_accounts.curator.clone(),
            market_accounts.user_collateral_ata.clone(),
            market_accounts.market_collateral_vault.clone(),
            market_accounts.apl_token_program.clone(),
            market_accounts.oracle.clone(),
            market_accounts.oracle.clone(),
        ];
        assert_eq!(
            BeginCapitalSweepAccounts::from_accounts(&mut account_infos.iter())
                .err()
                .unwrap(),
            LendingAccountValidationError::InvalidMarket
        );

        let account_infos = [
            market_accounts.market.clone(),
            market_accounts.borrow_position.clone(),
            market_accounts.curator.clone(),
            market_accounts.user_collateral_ata.clone(),
            other_accounts.market_collateral_vault.clone(),
            market_accounts.apl_token_program.clone(),
            market_accounts.oracle.clone(),
            market_accounts.oracle.clone(),
        ];
        assert_eq!(
            BeginCapitalSweepAccounts::from_accounts(&mut account_infos.iter())
                .err()
                .unwrap(),
            LendingAccountValidationError::InvalidMarketVault
        );
    }
}
