use arch_program::clock::Clock;
use autara_lib::ixs::UpdateConfigInstruction;

use crate::{error::LendingProgramResult, ixs::UpdateConfigAccounts};

#[inline(never)]
pub fn process_update_config(
    accounts: &UpdateConfigAccounts,
    data: &UpdateConfigInstruction,
    clock: &Clock,
) -> LendingProgramResult {
    let mut market_ref = accounts.market.load_mut();
    // Accrue interest under the current parameters first: sync_clock applies the fee
    // that is set at the time it runs to the whole elapsed window, so changing the fee
    // before syncing would apply the new fee retroactively to already-accrued interest.
    market_ref.sync_clock(clock.unix_timestamp)?;
    if let Some(supply_oracle_config) = &data.supply_oracle_config {
        market_ref.set_supply_oracle_config(*supply_oracle_config)?;
    }
    if let Some(collateral_oracle_config) = &data.collateral_oracle_config {
        market_ref.set_collateral_oracle_config(*collateral_oracle_config)?;
    }
    if let Some(ltv_config) = &data.ltv_config {
        market_ref.config_mut().update_ltv(ltv_config)?;
    }
    if let Some(max_utilisation_rate) = &data.max_utilisation_rate {
        market_ref
            .config_mut()
            .update_max_utilisation_rate(*max_utilisation_rate)?;
    }
    if let Some(max_supply_atoms) = &data.max_supply_atoms {
        market_ref
            .config_mut()
            .update_max_supply_atoms(*max_supply_atoms);
    }
    if let Some(fee) = &data.lending_market_fee_in_bps {
        market_ref.config_mut().set_lending_market_fee(*fee)?;
    }
    market_ref
        .config_mut()
        .sync_global_config(&accounts.global_config.load_ref());
    // check oracles are valid
    let _ = market_ref.wrapper_mut(
        accounts.updated_supply_oracle.try_into()?,
        accounts.updated_collateral_oracle.try_into()?,
        clock.unix_timestamp,
    )?;
    Ok(())
}
