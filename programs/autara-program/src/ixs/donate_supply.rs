use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::market::Market;
use autara_program_lib::accounts::packed::PackedOwnedAccount;
use autara_program_lib::accounts::program::Program;
use autara_program_lib::accounts::signer::Signer;
use autara_program_lib::accounts::token::{AplTokenProgram, TokenAccount};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::error::{LendingAccountValidationError, LendingProgramResult};
use crate::state::AutaraAccount;

pub struct DonateSupplyAccounts<'a, 'b> {
    pub market: ZeroCopyOwnedAccountMut<'a, 'b, AutaraAccount<Market>>,
    pub authority: Signer<'a, 'b>,
    pub authority_supply_ata: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub market_supply_vault: PackedOwnedAccount<'a, 'b, TokenAccount>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
}

impl<'a, 'b> DonateSupplyAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            market: next_account_info(accounts)?.try_into()?,
            authority: next_account_info(accounts)?.try_into()?,
            authority_supply_ata: next_account_info(accounts)?.try_into()?,
            market_supply_vault: next_account_info(accounts)?.try_into()?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        let market = self.market.load_ref();
        if market.supply_vault().vault() != self.market_supply_vault.key() {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        if &self.authority_supply_ata.mint != market.supply_vault().mint() {
            return Err(LendingAccountValidationError::InvalidMintForTokenAccount.into());
        }
        Ok(())
    }
}
