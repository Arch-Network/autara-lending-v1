use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::{AutaraEvent, DonateSupplyEvent},
    ixs::{log_ix, DonateSupplyInstruction},
};

use crate::{error::LendingProgramResult, ixs::DonateSupplyAccounts};

#[inline(never)]
pub fn process_donate_supply(
    donate_supply_accounts: &DonateSupplyAccounts,
    data: &DonateSupplyInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = donate_supply_accounts.market.load_mut();
    market_ref.sync_clock(clock.unix_timestamp)?;
    market_ref.donate_supply_atoms(data.amount)?;
    let seed = market_ref.seed();
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            donate_supply_accounts.market.key(),
            AutaraEvent::DonateSupply(DonateSupplyEvent {
                market: *donate_supply_accounts.market.key(),
                donor: *donate_supply_accounts.authority.key,
                mint: market_ref.supply_token_info().mint,
                amount: data.amount,
            }),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            donate_supply_accounts.authority_supply_ata.key(),
            donate_supply_accounts.market_supply_vault.key(),
            donate_supply_accounts.authority.key,
            &[],
            data.amount,
        )?,
        accounts,
        &[],
    )?;
    Ok(())
}
