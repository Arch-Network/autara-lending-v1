use arch_program::account::{next_account_info, AccountInfo};
use arch_program::program_error::ProgramError;
use autara_lib::state::market::Market;
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccount;

use crate::error::LendingProgramResult;
use crate::state::AutaraAccount;

pub struct LogAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccount<'a, 'b, AutaraAccount<Market>>,
}

impl<'a, 'b> LogAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self> {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
        };
        if !this.market.is_signer() {
            return Err(ProgramError::MissingRequiredSignature.into());
        }
        Ok(this)
    }
}
