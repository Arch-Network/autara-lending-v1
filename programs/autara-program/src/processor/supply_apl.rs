use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, SupplyAplInstruction},
};

use crate::{error::LendingProgramResult, ixs::SupplyAplAccounts};

#[inline(never)]
pub fn process_supply_apl(
    create_supply_position_accounts: &SupplyAplAccounts,
    data: &SupplyAplInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = create_supply_position_accounts.market.load_mut();
    let mut supply_position_ref = create_supply_position_accounts.supply_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        create_supply_position_accounts.supply_oracle.try_into()?,
        create_supply_position_accounts
            .collateral_oracle
            .try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    market_wrapper.lend(&mut supply_position_ref, data.amount)?;
    let seed = market_wrapper.market().seed();
    let supply_event = market_wrapper.get_single_market_transaction_event(
        create_supply_position_accounts.market.key(),
        create_supply_position_accounts.authority.key,
        create_supply_position_accounts.supply_position.key(),
        &market_wrapper.market().supply_token_info().mint,
        data.amount,
    )?;
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            create_supply_position_accounts.market.key(),
            AutaraEvent::Supply(supply_event),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            create_supply_position_accounts.authority_supply_ata.key(),
            create_supply_position_accounts.market_supply_vault.key(),
            create_supply_position_accounts.authority.key,
            &[],
            data.amount,
        )?,
        accounts,
        &[],
    )?;
    Ok(())
}
