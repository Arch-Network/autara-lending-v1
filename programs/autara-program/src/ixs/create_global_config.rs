use arch_program::account::{next_account_info, AccountInfo};
use autara_program_lib::accounts::{
    program::{Program, SystemProgram},
    signer::Signer,
};

use crate::error::LendingProgramResult;

pub struct CreateGlobalConfigAccounts<'a, 'b> {
    pub payer: Signer<'a, 'b>,
    pub global_config: &'b AccountInfo<'a>,
    pub system_program: Program<'a, 'b, SystemProgram>,
}

impl<'a, 'b> CreateGlobalConfigAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        Ok(Self {
            payer: next_account_info(accounts)?.try_into()?,
            global_config: next_account_info(accounts)?,
            system_program: next_account_info(accounts)?.try_into()?,
        })
    }
}
