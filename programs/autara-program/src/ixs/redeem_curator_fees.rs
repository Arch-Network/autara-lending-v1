use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::market::Market;
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

pub struct RedeemCuratorFeesAccounts<'a, 'b> {
    pub curator: Signer<'a, 'b>,
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub curator_supply_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
}

impl<'a, 'b> RedeemCuratorFeesAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            curator: next_account_info(accounts)?.try_into()?,
            market: next_account_info(accounts)?.try_into()?,
            curator_supply_ata: next_account_info(accounts)?.try_into()?,
            market_supply_vault: next_account_info(accounts)?.try_into()?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        let market = self.market.load_ref();
        if market.config().curator() != self.curator.key {
            return Err(LendingAccountValidationError::InvalidAuthority.into());
        }
        if market.supply_vault().vault() != self.market_supply_vault.key() {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::ixs::test_utils::AutaraAccounts;
    use autara_program_lib::accounts::AccountValidationError;

    #[test]
    pub fn validate_correct_accounts() {
        let account_set = AutaraAccounts::new();
        let accounts = [
            account_set.curator.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        RedeemCuratorFeesAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_fails_if_curator_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_b.user.clone(), // wrong curator
            account_set_a.market.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemCuratorFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.curator.clone(),
            account_set_a.market.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_b.market_supply_vault.clone(), // wrong vault
            account_set_a.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemCuratorFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarketVault);
    }

    #[test]
    pub fn validate_fails_if_curator_is_not_signer() {
        let mut account_set = AutaraAccounts::new();
        account_set.curator.non_signer();
        let accounts = [
            account_set.curator.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemCuratorFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::NotSigner);
    }

    #[test]
    pub fn validate_fails_if_market_is_not_owned_by_crate() {
        let mut account_set = AutaraAccounts::new();
        account_set.market.mutate_owner();
        let accounts = [
            account_set.curator.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemCuratorFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }
}
