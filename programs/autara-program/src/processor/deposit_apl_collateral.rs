use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, DepositAplCollateralInstruction},
};

use crate::{error::LendingProgramResult, ixs::DepositAplCollateralAccounts};

#[inline(never)]
pub fn process_deposit_apl_collateral(
    deposit_apl_collateral_accounts: &DepositAplCollateralAccounts,
    data: &DepositAplCollateralInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = deposit_apl_collateral_accounts.market.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        deposit_apl_collateral_accounts.supply_oracle.try_into()?,
        deposit_apl_collateral_accounts
            .collateral_oracle
            .try_into()?,
        clock.unix_timestamp,
    )?;

    market_wrapper.sync_clock(clock.unix_timestamp)?;

    let mut position_ref = deposit_apl_collateral_accounts.borrow_position.load_mut();

    market_wrapper.deposit_collateral(&mut position_ref, data.amount)?;

    let seed = market_wrapper.market().seed();
    let deposit_collateral_event = market_wrapper.get_single_market_transaction_event(
        deposit_apl_collateral_accounts.market.key(),
        deposit_apl_collateral_accounts.authority.key,
        deposit_apl_collateral_accounts.borrow_position.key(),
        &market_wrapper.market().collateral_token_info().mint,
        data.amount,
    )?;
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            deposit_apl_collateral_accounts.market.key(),
            AutaraEvent::DepositCollateral(deposit_collateral_event),
        ),
        accounts,
        &[&seed],
    )?;

    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            deposit_apl_collateral_accounts
                .authority_collateral_ata
                .key(),
            deposit_apl_collateral_accounts
                .market_collateral_vault
                .key(),
            deposit_apl_collateral_accounts.authority.key,
            &[],
            data.amount,
        )?,
        accounts,
        &[],
    )?;
    Ok(())
}
