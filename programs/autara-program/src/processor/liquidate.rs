use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    error::LendingError,
    event::{AutaraEvent, LiquidateEvent},
    ixs::{log_ix, LiquidateInstruction},
};

use crate::{error::LendingProgramResult, ixs::LiquidateAccounts};

#[inline(never)]
pub fn process_liquidate(
    liquidate_accounts: &LiquidateAccounts,
    data: &LiquidateInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = liquidate_accounts.market.load_mut();
    let mut borrow_position_ref = liquidate_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        liquidate_accounts.supply_oracle.try_into()?,
        liquidate_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;

    market_wrapper.sync_clock(clock.unix_timestamp)?;

    let liquidation =
        market_wrapper.liquidate(&mut borrow_position_ref, data.max_borrowed_atoms_to_repay)?;

    let total_collateral_liquidated = liquidation
        .liquidation_result_with_bonus
        .total_collateral_atoms_to_liquidate()?;
    if total_collateral_liquidated < data.min_collateral_atoms_to_receive {
        return Err(LendingError::LiquidationDidNotMeetRequirements.into());
    }

    let seed = market_wrapper.market().seed();

    let liquidation_event = LiquidateEvent {
        market: *liquidate_accounts.market.key(),
        liquidator: *liquidate_accounts.liquidator.key,
        liquidatee_position: *liquidate_accounts.borrow_position.key(),
        supply_mint: market_wrapper.market().supply_token_info().mint,
        collateral_mint: market_wrapper.market().collateral_token_info().mint,
        health_before_liquidation: liquidation.health_before_liquidation,
        health_after_liquidation: liquidation.health_after_liquidation,
        supply_repaid: liquidation
            .liquidation_result_with_bonus
            .borrowed_atoms_to_repay,
        collateral_liquidated: liquidation
            .liquidation_result_with_bonus
            .collateral_atoms_to_liquidate,
        liquidator_fee: liquidation
            .liquidation_result_with_bonus
            .collateral_atoms_liquidation_bonus,
    };

    invoke_signed_unchecked(
        &log_ix(
            program_id,
            liquidate_accounts.market.key(),
            AutaraEvent::Liquidate(liquidation_event),
        ),
        accounts,
        &[&seed],
    )?;

    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            liquidate_accounts.market_collateral_vault.key(),
            liquidate_accounts.liquidator_collateral_ata.key(),
            liquidate_accounts.market.key(),
            &[],
            total_collateral_liquidated,
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
            liquidate_accounts.liquidator_supply_ata.key(),
            liquidate_accounts.market_supply_vault.key(),
            liquidate_accounts.liquidator.key,
            &[],
            liquidation
                .liquidation_result_with_bonus
                .borrowed_atoms_to_repay,
        )?,
        accounts,
        &[],
    )?;

    Ok(())
}
