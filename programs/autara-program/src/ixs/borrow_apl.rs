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

pub struct BorrowAplAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub borrow_position: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<BorrowPosition>>,
    pub authority: Signer<'a, 'b>,
    pub authority_supply_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub supply_oracle: &'b AccountInfo<'a>,
    pub collateral_oracle: &'b AccountInfo<'a>,
}

impl<'a, 'b> BorrowAplAccounts<'a, 'b> {
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
            borrow_position: next_account_info(accounts)?.try_into()?,
            authority: next_account_info(accounts)?.try_into()?,
            authority_supply_ata: next_account_info(accounts)?.try_into()?,
            market_supply_vault: next_account_info(accounts)?.try_into()?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
            supply_oracle: next_account_info(accounts)?,
            collateral_oracle: next_account_info(accounts)?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        use crate::error::{format_pubkey_pair, validation_err};

        let borrow_position = self.borrow_position.load_ref();
        let market = self.market.load_ref();
        if borrow_position.authority() != self.authority.key {
            return Err(validation_err(
                LendingAccountValidationError::InvalidAuthority,
                format_pubkey_pair(
                    "position_authority",
                    borrow_position.authority(),
                    "signer",
                    self.authority.key,
                ),
            ));
        }
        if borrow_position.market() != self.market.key() {
            return Err(validation_err(
                LendingAccountValidationError::InvalidMarket,
                format_pubkey_pair(
                    "position_market",
                    borrow_position.market(),
                    "market",
                    self.market.key(),
                ),
            ));
        }
        if market.supply_vault().vault() != self.market_supply_vault.key() {
            return Err(validation_err(
                LendingAccountValidationError::InvalidMarketVault,
                format_pubkey_pair(
                    "expected_vault",
                    market.supply_vault().vault(),
                    "provided_vault",
                    self.market_supply_vault.key(),
                ),
            ));
        }
        if &self.authority_supply_ata.mint != market.supply_vault().mint() {
            return Err(validation_err(
                LendingAccountValidationError::InvalidMintForTokenAccount,
                format_pubkey_pair(
                    "ata_mint",
                    &self.authority_supply_ata.mint,
                    "market_mint",
                    market.supply_vault().mint(),
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use autara_program_lib::accounts::AccountValidationError;

    use super::*;
    use crate::ixs::test_utils::AutaraAccounts;

    #[test]
    pub fn validate_correct_accounts() {
        let account_set = AutaraAccounts::new();
        let accounts = [
            account_set.market.clone(),
            account_set.borrow_position.clone(),
            account_set.user.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
            account_set.oracle.clone(),
            account_set.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_fails_if_market_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_b.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarket);
    }

    #[test]
    pub fn validate_fails_if_authority_is_not_signer() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.user.non_signer();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::NotSigner);
    }

    #[test]
    pub fn validate_fails_if_market_is_not_owned_by_create() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.market.mutate_owner();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }

    #[test]
    pub fn validate_fails_if_position_is_not_owned_by_create() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.borrow_position.mutate_owner();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }

    #[test]
    pub fn validate_fails_if_authority_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_b.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidAuthority);
    }

    #[test]
    pub fn validate_fails_if_market_vault_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_b.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarketVault);
    }

    #[test]
    pub fn validate_fails_if_mint_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_b.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = BorrowAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(
            err,
            LendingAccountValidationError::InvalidMintForTokenAccount
        );
    }
}
