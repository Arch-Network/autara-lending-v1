use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::{global_config::GlobalConfig, market::Market};
use autara_program_lib::accounts::{
    packed::PackedOwnedAccount,
    program::Program,
    signer::Signer,
    token::{AplTokenProgram, TokenAccount},
    zero_copy::{ZeroCopyOwnedAccount, ZeroCopyOwnedAccountMut},
};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    state::AutaraAccount,
};

pub struct RedeemProtocolFeesAccounts<'a, 'b> {
    pub signer: Signer<'a, 'b>,
    pub global_config: ZeroCopyOwnedAccount<'a, 'b, AutaraAccount<GlobalConfig>>,
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub receiver_supply_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
}

impl<'a, 'b> RedeemProtocolFeesAccounts<'a, 'b> {
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
            market: next_account_info(accounts)?.try_into()?,
            receiver_supply_ata: next_account_info(accounts)?.try_into()?,
            market_supply_vault: next_account_info(accounts)?.try_into()?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        let (expected_global_config, _) = autara_lib::pda::find_global_config_pda(&crate::id());
        if *self.global_config.key() != expected_global_config {
            return Err(LendingAccountValidationError::InvalidProtocolAuthority.into());
        }
        let market = self.market.load_ref();
        let global_config = self.global_config.load_ref();
        if !global_config.can_redeem_fees(self.signer.key) {
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
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_fails_if_signer_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_b.user.clone(), // wrong signer
            account_set_a.global_config.clone(),
            account_set_a.market.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.global_admin.clone(),
            account_set_a.global_config.clone(),
            account_set_a.market.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_b.market_supply_vault.clone(), // wrong vault
            account_set_a.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarketVault);
    }

    #[test]
    pub fn validate_fails_if_global_config_is_not_canonical_pda() {
        let mut account_set = AutaraAccounts::new();
        // Replace with a non-canonical global config (different key, same data)
        let spoofed_global_config_data = autara_lib::state::global_config::GlobalConfig::new(
            *account_set.global_admin.key,
            *account_set.global_admin.key,
            0,
        );
        account_set.global_config = crate::ixs::test_utils::create_autara_account(
            arch_program::pubkey::Pubkey::new_unique(),
            spoofed_global_config_data,
        );
        let accounts = [
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidProtocolAuthority);
    }

    #[test]
    pub fn validate_fails_if_signer_is_not_signer() {
        let mut account_set = AutaraAccounts::new();
        account_set.global_admin.non_signer();
        let accounts = [
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set.global_admin.clone(),
            account_set.global_config.clone(),
            account_set.market.clone(),
            account_set.user_supply_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.apl_token_program.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }
}
