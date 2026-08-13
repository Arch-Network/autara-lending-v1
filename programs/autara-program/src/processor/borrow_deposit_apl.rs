use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, BorrowDepositAplInstruction},
};

use crate::{error::LendingProgramResult, ixs::BorrowDepositAplAccounts};

#[inline(never)]
pub fn process_borrow_deposit_apl(
    borrow_deposit_apl_accounts: &BorrowDepositAplAccounts,
    data: &BorrowDepositAplInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = borrow_deposit_apl_accounts.market.load_mut();
    let mut borrowing_position_ref = borrow_deposit_apl_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        borrow_deposit_apl_accounts.supply_oracle.try_into()?,
        borrow_deposit_apl_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    market_wrapper.deposit_collateral(&mut borrowing_position_ref, data.deposit_amount)?;
    market_wrapper.borrow(&mut borrowing_position_ref, data.borrow_amount)?;

    let seed = market_wrapper.market().seed();

    let event = market_wrapper.get_double_market_transaction_event(
        borrow_deposit_apl_accounts.market.key(),
        borrow_deposit_apl_accounts.authority.key,
        borrow_deposit_apl_accounts.borrow_position.key(),
        market_wrapper.market().collateral_vault().mint(),
        data.deposit_amount,
        market_wrapper.market().supply_vault().mint(),
        data.borrow_amount,
    )?;

    invoke_signed_unchecked(
        &log_ix(
            program_id,
            borrow_deposit_apl_accounts.market.key(),
            AutaraEvent::BorrowAndDeposit(event),
        ),
        accounts,
        &[&seed],
    )?;

    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            borrow_deposit_apl_accounts.market_supply_vault.key(),
            borrow_deposit_apl_accounts.authority_supply_ata.key(),
            borrow_deposit_apl_accounts.market.key(),
            &[],
            data.borrow_amount,
        )?,
        accounts,
        &[&seed],
    )?;

    if let Some(ix) = &data.ix_callback {
        invoke_signed_unchecked(ix, accounts, &[])?;
    }

    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            borrow_deposit_apl_accounts.authority_collateral_ata.key(),
            borrow_deposit_apl_accounts.market_collateral_vault.key(),
            borrow_deposit_apl_accounts.authority.key,
            &[],
            data.deposit_amount,
        )?,
        accounts,
        &[],
    )?;
    Ok(())
}
