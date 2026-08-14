use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::AutaraEvent,
    ixs::{log_ix, WithdrawAplCollateralInstruction},
};

use crate::{error::LendingProgramResult, ixs::WithdrawAplCollateralAccounts};

#[inline(never)]
pub fn process_withdraw_apl_collateral(
    withdraw_apl_collateral_accounts: &WithdrawAplCollateralAccounts,
    data: &WithdrawAplCollateralInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = withdraw_apl_collateral_accounts.market.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        withdraw_apl_collateral_accounts.supply_oracle.try_into()?,
        withdraw_apl_collateral_accounts
            .collateral_oracle
            .try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    let mut position_ref = withdraw_apl_collateral_accounts.borrow_position.load_mut();
    let atoms = if data.withdraw_all {
        position_ref.collateral_deposited_atoms()
    } else {
        data.amount
    };
    market_wrapper.withdraw_collateral(&mut position_ref, atoms)?;
    let seed = market_wrapper.market().seed();
    let withdraw_collateral_event = market_wrapper.get_single_market_transaction_event(
        withdraw_apl_collateral_accounts.market.key(),
        withdraw_apl_collateral_accounts.authority.key,
        withdraw_apl_collateral_accounts.borrow_position.key(),
        &market_wrapper.market().collateral_token_info().mint,
        atoms,
    )?;
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            withdraw_apl_collateral_accounts.market.key(),
            AutaraEvent::WithdrawCollateral(withdraw_collateral_event),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            withdraw_apl_collateral_accounts
                .market_collateral_vault
                .key(),
            withdraw_apl_collateral_accounts
                .authority_collateral_ata
                .key(),
            withdraw_apl_collateral_accounts.market.key(),
            &[],
            atoms,
        )?,
        accounts,
        &[&seed],
    )?;
    Ok(())
}
