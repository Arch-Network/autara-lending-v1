use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, WithdrawSupplyInstruction},
};

use crate::{error::LendingProgramResult, ixs::WithdrawSupplyAccounts};

#[inline(never)]
pub fn process_withdraw_supply(
    withdraw_supply_accounts: &WithdrawSupplyAccounts,
    data: &WithdrawSupplyInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = withdraw_supply_accounts.market.load_mut();
    let mut supply_position_ref = withdraw_supply_accounts.supply_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        withdraw_supply_accounts.supply_oracle.try_into()?,
        withdraw_supply_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    let withdraw_amount = if data.withdraw_all {
        market_wrapper.withdraw_all(&mut supply_position_ref)?
    } else {
        market_wrapper.withdraw(&mut supply_position_ref, data.amount)?;
        data.amount
    };
    let seed = market_wrapper.market().seed();
    let withdraw_event = market_wrapper.get_single_market_transaction_event(
        withdraw_supply_accounts.market.key(),
        withdraw_supply_accounts.authority.key,
        withdraw_supply_accounts.supply_position.key(),
        &market_wrapper.market().supply_token_info().mint,
        withdraw_amount,
    )?;
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            withdraw_supply_accounts.market.key(),
            AutaraEvent::Withdraw(withdraw_event),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            withdraw_supply_accounts.market_supply_vault.key(),
            withdraw_supply_accounts.authority_supply_ata.key(),
            withdraw_supply_accounts.market.key(),
            &[],
            withdraw_amount,
        )?,
        accounts,
        &[&seed],
    )?;
    Ok(())
}
