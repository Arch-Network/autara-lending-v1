use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::{
    pda::find_liquidator_whitelist_entry_pda,
    state::{
        borrow_position::BorrowPosition, liquidator_whitelist::LiquidatorWhitelistEntry,
        market::Market,
    },
};
use autara_program_lib::accounts::{
    packed::PackedOwnedAccount,
    program::Program,
    signer::Signer,
    token::{AplTokenProgram, BoxedTokenAccount},
    zero_copy::{ZeroCopyOwnedAccount, ZeroCopyOwnedAccountMut},
};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    state::AutaraAccount,
};

pub struct LiquidateAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub borrow_position: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<BorrowPosition>>,
    pub liquidator: Signer<'a, 'b>,
    pub liquidator_supply_ata: PackedOwnedAccount<'a, 'b, BoxedTokenAccount>,
    pub liquidator_collateral_ata: PackedOwnedAccount<'a, 'b, BoxedTokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, BoxedTokenAccount>,
    pub market_collateral_vault: PackedOwnedAccount<'a, 'b, BoxedTokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub supply_oracle: &'b AccountInfo<'a>,
    pub collateral_oracle: &'b AccountInfo<'a>,
    pub liquidator_whitelist_entry:
        Option<ZeroCopyOwnedAccount<'a, 'b, AutaraAccount<LiquidatorWhitelistEntry>>>,
}

impl<'a, 'b> LiquidateAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let mut this = Self {
            market: next_account_info(accounts)?.try_into()?,
            borrow_position: next_account_info(accounts)?.try_into()?,
            liquidator: next_account_info(accounts)?.try_into()?,
            liquidator_supply_ata: next_account_info(accounts)?.try_into()?,
            liquidator_collateral_ata: next_account_info(accounts)?.try_into()?,
            market_supply_vault: next_account_info(accounts)?.try_into()?,
            market_collateral_vault: next_account_info(accounts)?.try_into()?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
            supply_oracle: next_account_info(accounts)?,
            collateral_oracle: next_account_info(accounts)?,
            liquidator_whitelist_entry: None,
        };
        if !this
            .market
            .load_ref()
            .config()
            .liquidations_are_permissionless()
        {
            let entry_account = next_account_info(accounts)
                .map_err(|_| LendingAccountValidationError::MissingLiquidatorWhitelistEntry)?;
            if entry_account.key == &crate::id() {
                return Err(LendingAccountValidationError::MissingLiquidatorWhitelistEntry.into());
            }
            this.liquidator_whitelist_entry = Some(
                ZeroCopyOwnedAccount::<AutaraAccount<LiquidatorWhitelistEntry>>::try_from(
                    entry_account,
                )
                .map_err(|_| LendingAccountValidationError::LiquidatorNotWhitelisted)?,
            );
        }
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        let borrow_position = self.borrow_position.load_ref();
        let market = self.market.load_ref();
        if &self.liquidator_collateral_ata.mint != market.collateral_vault().mint() {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        if self.liquidator_supply_ata.mint != *market.supply_vault().mint() {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        if borrow_position.market() != self.market.key() {
            return Err(LendingAccountValidationError::InvalidMarket.into());
        }
        if self.market_collateral_vault.key() != market.collateral_vault().vault() {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        if self.market_supply_vault.key() != market.supply_vault().vault() {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        if !market.config().liquidations_are_permissionless() {
            let entry = self
                .liquidator_whitelist_entry
                .as_ref()
                .ok_or(LendingAccountValidationError::MissingLiquidatorWhitelistEntry)?;
            let (expected_entry, _) = find_liquidator_whitelist_entry_pda(
                &crate::id(),
                self.market.key(),
                self.liquidator.key,
            );
            let entry_ref = entry.load_ref();
            if entry.key() != &expected_entry
                || entry_ref.market() != self.market.key()
                || entry_ref.liquidator() != self.liquidator.key
                || !entry_ref.is_active()
            {
                return Err(LendingAccountValidationError::LiquidatorNotWhitelisted.into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use arch_program::pubkey::Pubkey;
    use autara_lib::{
        pda::find_liquidator_whitelist_entry_pda,
        state::{liquidator_whitelist::LiquidatorWhitelistEntry, market::Market},
    };
    use autara_program_lib::accounts::AccountValidationError;

    use super::*;
    use crate::{
        error::LendingAccountValidationError,
        ixs::test_utils::{create_autara_account, AutaraAccounts},
    };

    fn restrict_market(account_set: &AutaraAccounts) {
        let mut data = account_set.market.try_borrow_mut_data().unwrap();
        let market = bytemuck::from_bytes_mut::<Market>(&mut data);
        market
            .config_mut()
            .increment_active_whitelisted_liquidators()
            .unwrap();
    }

    fn whitelist_entry(
        market: Pubkey,
        liquidator: Pubkey,
        active: bool,
    ) -> crate::ixs::test_utils::AccountInfoWrapper {
        let (entry_key, bump) =
            find_liquidator_whitelist_entry_pda(&crate::id(), &market, &liquidator);
        let mut entry = LiquidatorWhitelistEntry::default();
        entry.initialize(market, liquidator, bump).unwrap();
        if !active {
            entry.deactivate().unwrap();
        }
        create_autara_account(entry_key, entry)
    }

    fn base_accounts(account_set: &AutaraAccounts) -> Vec<AccountInfo<'static>> {
        vec![
            account_set.market.clone(),
            account_set.borrow_position.clone(),
            account_set.user.clone(),
            account_set.user_supply_ata.clone(),
            account_set.user_collateral_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.market_collateral_vault.clone(),
            account_set.apl_token_program.clone(),
            account_set.oracle.clone(),
            account_set.oracle.clone(),
        ]
    }

    #[test]
    fn restricted_market_accepts_active_entry_for_liquidator_signer() {
        let account_set = AutaraAccounts::new();
        restrict_market(&account_set);
        let entry = whitelist_entry(*account_set.market.key, *account_set.user.key, true);
        let mut accounts = base_accounts(&account_set);
        accounts.push(entry.clone());

        LiquidateAccounts::from_accounts(&mut accounts.iter()).unwrap();
    }

    #[test]
    fn restricted_market_rejects_missing_whitelist_entry() {
        let account_set = AutaraAccounts::new();
        restrict_market(&account_set);
        let accounts = base_accounts(&account_set);

        let result = LiquidateAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected missing whitelist entry");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::MissingLiquidatorWhitelistEntry
        );
    }

    #[test]
    fn restricted_market_rejects_inactive_whitelist_entry() {
        let account_set = AutaraAccounts::new();
        restrict_market(&account_set);
        let entry = whitelist_entry(*account_set.market.key, *account_set.user.key, false);
        let mut accounts = base_accounts(&account_set);
        accounts.push(entry.clone());

        let result = LiquidateAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected inactive whitelist entry to fail");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::LiquidatorNotWhitelisted
        );
    }

    #[test]
    fn restricted_market_rejects_entry_for_another_liquidator() {
        let account_set = AutaraAccounts::new();
        restrict_market(&account_set);
        let entry = whitelist_entry(*account_set.market.key, Pubkey::new_unique(), true);
        let mut accounts = base_accounts(&account_set);
        accounts.push(entry.clone());

        let result = LiquidateAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected another liquidator's entry to fail");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::LiquidatorNotWhitelisted
        );
    }

    #[test]
    fn restricted_market_rejects_entry_for_another_market() {
        let account_set = AutaraAccounts::new();
        restrict_market(&account_set);
        let entry = whitelist_entry(Pubkey::new_unique(), *account_set.user.key, true);
        let mut accounts = base_accounts(&account_set);
        accounts.push(entry.clone());

        let result = LiquidateAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected another market's entry to fail");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::LiquidatorNotWhitelisted
        );
    }

    #[test]
    pub fn validate_correct_accounts() {
        let account_set = AutaraAccounts::new();
        let accounts = [
            account_set.market.clone(),
            account_set.borrow_position.clone(),
            account_set.user.clone(),
            account_set.user_supply_ata.clone(),
            account_set.user_collateral_ata.clone(),
            account_set.market_supply_vault.clone(),
            account_set.market_collateral_vault.clone(),
            account_set.apl_token_program.clone(),
            account_set.oracle.clone(),
            account_set.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter()).unwrap();
    }

    #[test]
    pub fn validate_fails_if_market_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_b.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_b.user_supply_ata.clone(),
            account_set_b.user_collateral_ata.clone(),
            account_set_b.market_supply_vault.clone(),
            account_set_b.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarket);
    }

    #[test]
    pub fn validate_fails_if_liquidator_is_not_signer() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.user.non_signer();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.user_collateral_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
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
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.user_collateral_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }

    #[test]
    pub fn validate_fails_if_position_is_not_owned_by_crate() {
        let mut account_set_a = AutaraAccounts::new();
        account_set_a.borrow_position.mutate_owner();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.user_collateral_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, AccountValidationError::InvalidOwner);
    }

    #[test]
    pub fn validate_fails_if_collateral_mint_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_b.user_collateral_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(
            err,
            LendingAccountValidationError::InvalidMintForTokenAccount
        );
    }

    #[test]
    pub fn validate_fails_if_supply_mint_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_b.user_supply_ata.clone(),
            account_set_a.user_collateral_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_a.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(
            err,
            LendingAccountValidationError::InvalidMintForTokenAccount
        );
    }

    #[test]
    pub fn validate_fails_if_market_collateral_vault_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.user_collateral_ata.clone(),
            account_set_a.market_supply_vault.clone(),
            account_set_b.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarketVault);
    }

    #[test]
    pub fn validate_fails_if_market_supply_vault_mismatch() {
        let account_set_a = AutaraAccounts::new();
        let account_set_b = AutaraAccounts::new();
        let accounts = [
            account_set_a.market.clone(),
            account_set_a.borrow_position.clone(),
            account_set_a.user.clone(),
            account_set_a.user_supply_ata.clone(),
            account_set_a.user_collateral_ata.clone(),
            account_set_b.market_supply_vault.clone(),
            account_set_a.market_collateral_vault.clone(),
            account_set_a.apl_token_program.clone(),
            account_set_a.oracle.clone(),
            account_set_a.oracle.clone(),
        ];
        let accounts_iter = accounts.iter();
        let result = LiquidateAccounts::from_accounts(&mut accounts_iter.into_iter());
        let Err(err) = result else {
            panic!("Expected an error, but got Ok");
        };
        assert_eq!(err, LendingAccountValidationError::InvalidMarketVault);
    }
}
