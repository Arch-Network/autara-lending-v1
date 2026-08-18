use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::market::Market;
use autara_program_lib::accounts::{
    program::{Program, SystemProgram},
    signer::Signer,
    zero_copy::ZeroCopyOwnedAccountMut,
};

use crate::{error::LendingProgramResult, state::AutaraAccount};

pub struct CreateBorrowPositionAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub borrow_position: &'b AccountInfo<'a>,
    pub authority: Signer<'a, 'b>,
    pub payer: Signer<'a, 'b>,
    pub system_program: Program<'a, 'b, SystemProgram>,
}

impl<'a, 'b> CreateBorrowPositionAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        Ok(Self {
            market: next_account_info(accounts)?.try_into()?,
            borrow_position: next_account_info(accounts)?,
            authority: next_account_info(accounts)?.try_into()?,
            payer: next_account_info(accounts)?.try_into()?,
            system_program: next_account_info(accounts)?.try_into()?,
        })
    }
}
