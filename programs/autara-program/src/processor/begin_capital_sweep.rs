use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::{AutaraEvent, CapitalSweepStartedEvent},
    ixs::{log_ix, BeginCapitalSweepInstruction},
};

use crate::{error::LendingProgramResult, ixs::BeginCapitalSweepAccounts};

pub fn process_begin_capital_sweep(
    sweep_accounts: &BeginCapitalSweepAccounts,
    _data: &BeginCapitalSweepInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = sweep_accounts.market.load_mut();
    let mut borrow_position_ref = sweep_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        sweep_accounts.supply_oracle.try_into()?,
        sweep_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    market_wrapper.sync_clock(clock.unix_timestamp)?;
    let health_before_sweep = market_wrapper.begin_capital_sweep(&mut borrow_position_ref)?;
    let collateral_swept = borrow_position_ref.swept_collateral_atoms();
    let collateral_mint = market_wrapper.market().collateral_token_info().mint;
    let seed = market_wrapper.market().seed();

    invoke_signed_unchecked(
        &log_ix(
            program_id,
            sweep_accounts.market.key(),
            AutaraEvent::CapitalSweepStarted(CapitalSweepStartedEvent {
                market: *sweep_accounts.market.key(),
                curator: *sweep_accounts.curator.key,
                position: *sweep_accounts.borrow_position.key(),
                collateral_mint,
                health_before_sweep,
                collateral_swept,
            }),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            sweep_accounts.market_collateral_vault.key(),
            sweep_accounts.curator_collateral_ata.key(),
            sweep_accounts.market.key(),
            &[],
            collateral_swept,
        )?,
        accounts,
        &[&seed],
    )?;
    Ok(())
}
