use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::{AutaraEvent, CapitalSweepSettledEvent},
    ixs::{log_ix, SettleCapitalSweepInstruction},
};

use crate::{error::LendingProgramResult, ixs::SettleCapitalSweepAccounts};

#[inline(never)]
pub fn process_settle_capital_sweep(
    settle_accounts: &SettleCapitalSweepAccounts,
    data: &SettleCapitalSweepInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = settle_accounts.market.load_mut();
    let mut borrow_position_ref = settle_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        settle_accounts.supply_oracle.try_into()?,
        settle_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    let settlement = market_wrapper.settle_capital_sweep(
        &mut borrow_position_ref,
        data.max_borrowed_atoms_to_repay,
        data.max_collateral_atoms_to_return,
    )?;
    let liquidation = settlement.liquidation_result_with_bonus;
    let supply_repaid = liquidation.borrowed_atoms_to_repay;
    let collateral_returned = settlement.collateral_atoms_returned;
    let supply_mint = market_wrapper.market().supply_token_info().mint;
    let collateral_mint = market_wrapper.market().collateral_token_info().mint;
    let seed = market_wrapper.market().seed();

    invoke_signed_unchecked(
        &log_ix(
            program_id,
            settle_accounts.market.key(),
            AutaraEvent::CapitalSweepSettled(CapitalSweepSettledEvent {
                market: *settle_accounts.market.key(),
                curator: *settle_accounts.curator.key,
                position: *settle_accounts.borrow_position.key(),
                supply_mint,
                collateral_mint,
                health_before_settlement: settlement.health_before_settlement,
                health_after_settlement: settlement.health_after_settlement,
                supply_repaid,
                collateral_liquidated: liquidation.collateral_atoms_to_liquidate,
                curator_bonus: liquidation.collateral_atoms_liquidation_bonus,
                collateral_returned,
            }),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            settle_accounts.curator_supply_ata.key(),
            settle_accounts.market_supply_vault.key(),
            settle_accounts.curator.key,
            &[],
            supply_repaid,
        )?,
        accounts,
        &[],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            settle_accounts.curator_collateral_ata.key(),
            settle_accounts.market_collateral_vault.key(),
            settle_accounts.curator.key,
            &[],
            collateral_returned,
        )?,
        accounts,
        &[],
    )?;
    Ok(())
}
