use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
};
use autara_lib::{
    event::{AutaraEvent, ReedeemFeeEvent},
    ixs::log_ix,
};

use crate::{error::LendingProgramResult, ixs::redeem_protocol_fees::RedeemProtocolFeesAccounts};

#[inline(never)]
pub fn process_redeem_protocol_fees(
    accounts: &RedeemProtocolFeesAccounts,
    account_infos: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = accounts.market.load_mut();
    market_ref.sync_clock(clock.unix_timestamp)?;
    let to_withdraw = market_ref.redeem_protocol_fees()?;
    let seed = market_ref.seed();
    invoke_signed_unchecked(
        &log_ix(
            program_id,
            accounts.market.key(),
            AutaraEvent::ReedeemCuratorFees(ReedeemFeeEvent {
                market: *accounts.market.key(),
                fee_receiver: accounts.receiver_supply_ata.owner,
                fee_amount: to_withdraw,
                mint: *market_ref.supply_vault().mint(),
                supply_vault_snapshot: market_ref.supply_vault().get_summary()?,
            }),
        ),
        account_infos,
        &[&seed],
    )?;
    invoke_signed_unchecked(
        &apl_token::instruction::transfer(
            &apl_token::id(),
            accounts.market_supply_vault.key(),
            accounts.receiver_supply_ata.key(),
            accounts.market.key(),
            &[],
            to_withdraw,
        )?,
        account_infos,
        &[&seed],
    )?;

    Ok(())
}
