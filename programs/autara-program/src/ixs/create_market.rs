use apl_token::state::Mint;
use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::global_config::GlobalConfig;
use autara_program_lib::accounts::{
    packed::PackedOwnedAccount,
    program::{Program, SystemProgram},
    signer::Signer,
    token::{AplAssociatedTokenProgram, AplTokenProgram},
    zero_copy::ZeroCopyOwnedAccount,
};

use crate::{error::LendingProgramResult, state::AutaraAccount};

pub struct CreateMarketAccounts<'a, 'b> {
    pub curator: Signer<'a, 'b>,
    pub payer: Signer<'a, 'b>,
    pub global_config: ZeroCopyOwnedAccount<'a, 'b, AutaraAccount<GlobalConfig>>,
    pub market: &'b AccountInfo<'a>,
    pub supply_mint: PackedOwnedAccount<'a, 'b, Mint>,
    pub supply_vault: &'b AccountInfo<'a>,
    pub collateral_mint: PackedOwnedAccount<'a, 'b, Mint>,
    pub collateral_vault: &'b AccountInfo<'a>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub associated_token_program: Program<'a, 'b, AplAssociatedTokenProgram>,
    pub system_program: Program<'a, 'b, SystemProgram>,
}

impl<'a, 'b> CreateMarketAccounts<'a, 'b> {
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            curator: next_account_info(accounts)?.try_into()?,
            payer: next_account_info(accounts)?.try_into()?,
            global_config: next_account_info(accounts)?.try_into()?,
            market: next_account_info(accounts)?,
            supply_mint: next_account_info(accounts)?.try_into()?,
            supply_vault: next_account_info(accounts)?,
            collateral_mint: next_account_info(accounts)?.try_into()?,
            collateral_vault: next_account_info(accounts)?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
            associated_token_program: next_account_info(accounts)?.try_into()?,
            system_program: next_account_info(accounts)?.try_into()?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        let (expected_global_config, _) = autara_lib::pda::find_global_config_pda(&crate::id());
        if *self.global_config.key() != expected_global_config {
            return Err(
                crate::error::LendingAccountValidationError::InvalidProtocolAuthority.into(),
            );
        }
        Ok(())
    }
}
