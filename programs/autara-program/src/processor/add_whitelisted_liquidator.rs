use arch_program::{
    account::AccountInfo, program::invoke_signed_unchecked, pubkey::Pubkey, rent::minimum_rent,
    system_instruction,
};
use autara_lib::{
    event::{AutaraEvent, LiquidatorWhitelistUpdatedEvent},
    ixs::{log_ix, AddWhitelistedLiquidatorInstruction},
    pda::liquidator_whitelist_entry_seed_with_bump,
    state::liquidator_whitelist::LiquidatorWhitelistEntry,
};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::{
    error::LendingProgramResult,
    ixs::AddWhitelistedLiquidatorAccounts,
    state::{AutaraAccount, AutaraUninitializedAccount},
};

pub fn process_add_whitelisted_liquidator(
    add_accounts: &AddWhitelistedLiquidatorAccounts,
    data: &AddWhitelistedLiquidatorInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
) -> LendingProgramResult {
    if add_accounts.whitelist_entry.owner != program_id {
        let bump = [data.bump];
        let seed = liquidator_whitelist_entry_seed_with_bump(
            add_accounts.market.key(),
            &data.liquidator,
            &bump,
        );
        invoke_signed_unchecked(
            &system_instruction::create_account(
                add_accounts.curator.key,
                add_accounts.whitelist_entry.key,
                minimum_rent(std::mem::size_of::<LiquidatorWhitelistEntry>()),
                std::mem::size_of::<LiquidatorWhitelistEntry>() as u64,
                program_id,
            ),
            accounts,
            &[&seed],
        )?;
        let entry = ZeroCopyOwnedAccountMut::<
            AutaraUninitializedAccount<LiquidatorWhitelistEntry>,
        >::try_from(add_accounts.whitelist_entry)?;
        entry
            .load_mut()
            .initialize(*add_accounts.market.key(), data.liquidator, data.bump)?;
    } else {
        let entry = ZeroCopyOwnedAccountMut::<AutaraAccount<LiquidatorWhitelistEntry>>::try_from(
            add_accounts.whitelist_entry,
        )?;
        entry.load_mut().activate()?;
    }

    let mut market = add_accounts.market.load_mut();
    market
        .config_mut()
        .increment_active_whitelisted_liquidators()?;
    let event = LiquidatorWhitelistUpdatedEvent {
        market: *add_accounts.market.key(),
        curator: *add_accounts.curator.key,
        liquidator: data.liquidator,
        active_whitelisted_liquidators: market.config().active_whitelisted_liquidators(),
    };
    let market_seed = market.seed();
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            add_accounts.market.key(),
            AutaraEvent::LiquidatorWhitelistEntryAdded(event),
        ),
        accounts,
        &[&market_seed],
    )?;
    Ok(())
}
