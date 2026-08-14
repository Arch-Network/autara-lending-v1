use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::{market::Market, supply_position::SupplyPosition};
use autara_program_lib::accounts::packed::PackedOwnedAccount;
use autara_program_lib::accounts::program::Program;
use autara_program_lib::accounts::signer::Signer;
use autara_program_lib::accounts::token::{AplTokenProgram, TokenAccount};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::error::{LendingAccountValidationError, LendingProgramResult};
use crate::state::AutaraAccount;

pub struct SupplyAplAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub supply_position: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<SupplyPosition>>,
    pub authority: Signer<'a, 'b>,
    pub authority_supply_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub supply_oracle: &'b AccountInfo<'a>,
    pub collateral_oracle: &'b AccountInfo<'a>,
}

impl<'a, 'b> SupplyAplAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
            supply_position: next_account_info(accounts)?.try_into()?,
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
        let supply_position = self.supply_position.load_ref();
        let market = self.market.load_ref();
        if supply_position.authority() != self.authority.key {
            return Err(LendingAccountValidationError::InvalidAuthority.into());
        }
        if supply_position.market() != self.market.key() {
            return Err(LendingAccountValidationError::InvalidMarket.into());
        }
        if market.supply_vault().vault() != self.market_supply_vault.key() {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        if &self.authority_supply_ata.mint != market.supply_vault().mint() {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
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
            account_set.supply_position.clone(),
            account_set.user.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
            account_set.oracle.clone(),
            account_set.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_fails_if_market_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_b.market.clone(),
            account_set_a.supply_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.supply_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::NotSigner);
    }

    #[test]
    pub fn validate_fails_if_market_is_not_owned_by_crate() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.market.mutate_owner();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.supply_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }

    #[test]
    pub fn validate_fails_if_position_is_not_owned_by_crate() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.supply_position.mutate_owner();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.supply_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.supply_position.clone(),
            account_set_b.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.supply_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_b.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.supply_position.clone(),
            account_set_a.user.clone(),
            account_set_b.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = SupplyAplAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(
            err,
            LendingAccountValidationError::InvalidMintForTokenAccount
        );
    }
}
