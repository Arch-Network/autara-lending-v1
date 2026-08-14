use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::global_config::GlobalConfig;
use autara_program_lib::accounts::{signer::Signer, zero_copy::ZeroCopyOwnedAccountMut};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    state::AutaraAccount,
};

pub struct UpdateGlobalConfigAccounts<'a, 'b> {
    pub signer: Signer<'a, 'b>,
    pub global_config: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<GlobalConfig>>,
}

impl<'a, 'b> UpdateGlobalConfigAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            signer: next_account_info(accounts)?.try_into()?,
            global_config: next_account_info(accounts)?.try_into()?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult {
        let config = self.global_config.load_ref();
        if !config.can_update_config(self.signer.key)
            && !config.can_upgrade_nomination(self.signer.key)
        {
            return Err(LendingAccountValidationError::InvalidProtocolAuthority.into());
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
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
        ];
        let accounts_iter = accounts.iter();
        UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_correct_nominated_admin() {
        let account_set = AutaraAccounts::new();
        let accounts = [
            account_set.nominated_admin.clone(),
            account_set.global_config.clone(),
        ];
        let accounts_iter = accounts.iter();
        UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_fails_if_admin_is_not_signer() {
        let mut account_set = AutaraAccounts::new();
        account_set.global_admin.non_signer();
        let accounts = [
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::NotSigner);
    }

    #[test]
    pub fn validate_fails_if_nominated_is_not_signer() {
        let mut account_set = AutaraAccounts::new();
        account_set.nominated_admin.non_signer();
        let accounts = [
            account_set.nominated_admin.clone(),
            account_set.global_config.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::NotSigner);
    }

    #[test]
    pub fn validate_fails_if_global_config_is_not_owned_by_crate() {
        let mut account_set = AutaraAccounts::new();
        account_set.global_config.mutate_owner();
        let accounts = [
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }

    #[test]
    pub fn validate_fails_if_admin_lacks_authority() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.global_admin.clone(),
            account_set_b.global_config.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidProtocolAuthority);
    }
}
