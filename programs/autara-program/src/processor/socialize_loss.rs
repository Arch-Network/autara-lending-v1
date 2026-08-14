use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::{AutaraEvent, SocializeLossEvent},
    ixs::{log_ix, SocializeLossInstruction},
};

use crate::{error::LendingProgramResult, ixs::SocializeLossAccounts};

#[inline(never)]
pub fn process_socialize_loss(
    socialize_loss_accounts: &SocializeLossAccounts,
    _data: &SocializeLossInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = socialize_loss_accounts.market.load_mut();
    let mut borrow_position_ref = socialize_loss_accounts.borrow_position.load_mut();
    let mut market_wrapper = market_ref.wrapper_mut(
        socialize_loss_accounts.supply_oracle.try_into()?,
        socialize_loss_accounts.collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;

    market_wrapper.sync_clock(clock.unix_timestamp)?;

    let (debt_socialized, collateral_liquidated) =
        market_wrapper.socialize_loss(&mut borrow_position_ref)?;

    let seed = market_wrapper.market().seed();

    let socialize_loss_event = SocializeLossEvent {
        market: *socialize_loss_accounts.market.key(),
        position: *socialize_loss_accounts.borrow_position.key(),
        debt_socialized,
        collateral_liquidated,
    };

    invoke_signed_unchecked(
        &log_ix(
            program_id,
            socialize_loss_accounts.market.key(),
            AutaraEvent::SocializeLoss(socialize_loss_event),
        ),
        accounts,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            socialize_loss_accounts.market_collateral_vault.key(),
            socialize_loss_accounts.receiver_collateral_ata.key(),
            socialize_loss_accounts.market.key(),
            &[],
            collateral_liquidated,
        )?,
        accounts,
        &[&seed],
    )?;

    Ok(())
}
