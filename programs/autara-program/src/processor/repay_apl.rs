use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, RepayAplInstruction},
};

use crate::{error::LendingProgramResult, ixs::RepayAplAccounts};

#[inline(never)]
pub fn process_repay_apl(
    repay_apl_accounts: &RepayAplAccounts,
    data: &RepayAplInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = repay_apl_accounts.market.load_mut();
    let mut borrowing_position_ref = repay_apl_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        repay_apl_accounts.supply_oracle.try_into()?,
        repay_apl_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    let amount = if data.repay_all {
        market_wrapper.repay_all(&mut borrowing_position_ref)?
    } else {
        market_wrapper.repay(&mut borrowing_position_ref, data.amount)?;
        data.amount
    };
    let seed = market_wrapper.market().seed();
    let repay_event = market_wrapper.get_single_market_transaction_event(
        repay_apl_accounts.market.key(),
        repay_apl_accounts.authority.key,
        repay_apl_accounts.borrow_position.key(),
        &market_wrapper.market().supply_token_info().mint,
        amount,
    )?;
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            repay_apl_accounts.market.key(),
            AutaraEvent::Repay(repay_event),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            repay_apl_accounts.authority_supply_ata.key(),
            repay_apl_accounts.market_supply_vault.key(),
            repay_apl_accounts.authority.key,
            &[],
            amount,
        )?,
        accounts,
        &[],
    )?;
    Ok(())
}
