use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::{
    ixs::{AddWhitelistedLiquidatorInstruction, RemoveWhitelistedLiquidatorInstruction},
    pda::find_liquidator_whitelist_entry_pda,
    state::{liquidator_whitelist::LiquidatorWhitelistEntry, market::Market},
};
use autara_program_lib::accounts::{
    program::{Program, SystemProgram},
    signer::Signer,
    zero_copy::{ZeroCopyOwnedAccount, ZeroCopyOwnedAccountMut},
};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    state::AutaraAccount,
};

pub struct AddWhitelistedLiquidatorAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub curator: Signer<'a, 'b>,
    pub whitelist_entry: &'b AccountInfo<'a>,
    pub system_program: Program<'a, 'b, SystemProgram>,
}

impl<'a, 'b> AddWhitelistedLiquidatorAccounts<'a, 'b> {
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
        data: &AddWhitelistedLiquidatorInstruction,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
            curator: next_account_info(accounts)?.try_into()?,
            whitelist_entry: next_account_info(accounts)?,
            system_program: next_account_info(accounts)?.try_into()?,
        };
        this.validate(data)?;
        Ok(this)
    }

    fn validate(&self, data: &AddWhitelistedLiquidatorInstruction) -> LendingProgramResult {
        let market = self.market.load_ref();
        if market.config().curator() != self.curator.key {
            return Err(LendingAccountValidationError::InvalidMarketAuthority.into());
        }
        let (expected_entry, expected_bump) =
            find_liquidator_whitelist_entry_pda(&crate::id(), self.market.key(), &data.liquidator);
        if self.whitelist_entry.key != &expected_entry || data.bump != expected_bump {
            return Err(LendingAccountValidationError::InvalidLiquidatorWhitelistEntry.into());
        }
        if self.whitelist_entry.owner == &crate::id() {
            let entry = ZeroCopyOwnedAccount::<AutaraAccount<LiquidatorWhitelistEntry>>::try_from(
                self.whitelist_entry,
            )?;
            let entry = entry.load_ref();
            if entry.market() != self.market.key() || entry.liquidator() != &data.liquidator {
                return Err(LendingAccountValidationError::InvalidLiquidatorWhitelistEntry.into());
            }
        } else if self.whitelist_entry.owner != &arch_program::system_program::SYSTEM_PROGRAM_ID {
            return Err(LendingAccountValidationError::InvalidLiquidatorWhitelistEntry.into());
        }
        Ok(())
    }
}

pub struct RemoveWhitelistedLiquidatorAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub curator: Signer<'a, 'b>,
    pub whitelist_entry: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<LiquidatorWhitelistEntry>>,
}

impl<'a, 'b> RemoveWhitelistedLiquidatorAccounts<'a, 'b> {
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
        data: &RemoveWhitelistedLiquidatorInstruction,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
            curator: next_account_info(accounts)?.try_into()?,
            whitelist_entry: next_account_info(accounts)?.try_into()?,
        };
        this.validate(data)?;
        Ok(this)
    }

    fn validate(&self, data: &RemoveWhitelistedLiquidatorInstruction) -> LendingProgramResult {
        let market = self.market.load_ref();
        if market.config().curator() != self.curator.key {
            return Err(LendingAccountValidationError::InvalidMarketAuthority.into());
        }
        let (expected_entry, _) =
            find_liquidator_whitelist_entry_pda(&crate::id(), self.market.key(), &data.liquidator);
        let entry = self.whitelist_entry.load_ref();
        if self.whitelist_entry.key() != &expected_entry
            || entry.market() != self.market.key()
            || entry.liquidator() != &data.liquidator
        {
            return Err(LendingAccountValidationError::InvalidLiquidatorWhitelistEntry.into());
        }
        Ok(())
    }
}
