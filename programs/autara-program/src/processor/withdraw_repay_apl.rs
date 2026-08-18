use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, WithdrawRepayAplInstruction},
};

use crate::{error::LendingProgramResult, ixs::WithdrawRepayAplAccounts};

#[inline(never)]
pub fn process_withdraw_repay_apl(
    withdraw_repay_apl_accounts: &WithdrawRepayAplAccounts,
    data: &WithdrawRepayAplInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = withdraw_repay_apl_accounts.market.load_mut();
    let mut borrowing_position_ref = withdraw_repay_apl_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        withdraw_repay_apl_accounts.supply_oracle.try_into()?,
        withdraw_repay_apl_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    let repay_amount = if data.repay_all {
        market_wrapper.repay_all(&mut borrowing_position_ref)?
    } else {
        market_wrapper.repay(&mut borrowing_position_ref, data.repay_amount)?;
        data.repay_amount
    };
    let withdraw_amount = if data.withdraw_all {
        borrowing_position_ref.collateral_deposited_atoms()
    } else {
        data.withdraw_amount
    };
    market_wrapper.withdraw_collateral(&mut borrowing_position_ref, withdraw_amount)?;

    let seed = market_wrapper.market().seed();
    let event = market_wrapper.get_double_market_transaction_event(
        withdraw_repay_apl_accounts.market.key(),
        withdraw_repay_apl_accounts.authority.key,
        withdraw_repay_apl_accounts.borrow_position.key(),
        market_wrapper.market().supply_vault().mint(),
        repay_amount,
        market_wrapper.market().collateral_vault().mint(),
        withdraw_amount,
    )?;

    invoke_signed_unchecked(
        &log_ix(
            program_id,
            withdraw_repay_apl_accounts.market.key(),
            AutaraEvent::WithdrawAndRepay(event),
        ),
        accounts,
        &[&seed],
    )?;

    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            withdraw_repay_apl_accounts.market_collateral_vault.key(),
            withdraw_repay_apl_accounts.authority_collateral_ata.key(),
            withdraw_repay_apl_accounts.market.key(),
            &[],
            withdraw_amount,
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
            withdraw_repay_apl_accounts.authority_supply_ata.key(),
            withdraw_repay_apl_accounts.market_supply_vault.key(),
            withdraw_repay_apl_accounts.authority.key,
            &[],
            repay_amount,
        )?,
        accounts,
        &[],
    )?;

    Ok(())
}
