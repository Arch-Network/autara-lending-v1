use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, BorrowAplInstruction},
};

use crate::{error::LendingProgramResult, ixs::BorrowAplAccounts};

#[inline(never)]
pub fn process_borrow_apl(
    borrow_apl_accounts: &BorrowAplAccounts,
    data: &BorrowAplInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = borrow_apl_accounts.market.load_mut();
    let mut borrowing_position_ref = borrow_apl_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        borrow_apl_accounts.supply_oracle.try_into()?,
        borrow_apl_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    market_wrapper.borrow(&mut borrowing_position_ref, data.amount)?;
    let seed = market_wrapper.market().seed();
    let borrow_event = market_wrapper.get_single_market_transaction_event(
        borrow_apl_accounts.market.key(),
        borrow_apl_accounts.authority.key,
        borrow_apl_accounts.borrow_position.key(),
        &market_wrapper.market().supply_token_info().mint,
        data.amount,
    )?;
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            borrow_apl_accounts.market.key(),
            AutaraEvent::Borrow(borrow_event),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            borrow_apl_accounts.market_supply_vault.key(),
            borrow_apl_accounts.authority_supply_ata.key(),
            borrow_apl_accounts.market.key(),
            &[],
            data.amount,
        )?,
        accounts,
        &[&seed],
    )?;
    Ok(())
}
