use arch_program::{account::AccountInfo, program::invoke_signed_unchecked, pubkey::Pubkey};
use autara_lib::{
    event::{AutaraEvent, LiquidatorWhitelistUpdatedEvent},
    ixs::{log_ix, RemoveWhitelistedLiquidatorInstruction},
};

use crate::{error::LendingProgramResult, ixs::RemoveWhitelistedLiquidatorAccounts};

pub fn process_remove_whitelisted_liquidator(
    remove_accounts: &RemoveWhitelistedLiquidatorAccounts,
    data: &RemoveWhitelistedLiquidatorInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
) -> LendingProgramResult {
    remove_accounts.whitelist_entry.load_mut().deactivate()?;

    let mut market = remove_accounts.market.load_mut();
    market
        .config_mut()
        .decrement_active_whitelisted_liquidators()?;
    let event = LiquidatorWhitelistUpdatedEvent {
        market: *remove_accounts.market.key(),
        curator: *remove_accounts.curator.key,
        liquidator: data.liquidator,
        active_whitelisted_liquidators: market.config().active_whitelisted_liquidators(),
    };
    let market_seed = market.seed();
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            remove_accounts.market.key(),
            AutaraEvent::LiquidatorWhitelistEntryRemoved(event),
        ),
        accounts,
        &[&market_seed],
    )?;
    Ok(())
}
