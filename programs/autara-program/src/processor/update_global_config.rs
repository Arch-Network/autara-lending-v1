use autara_lib::{error::LendingError, ixs::UpdateGlobalConfigInstruction};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    ixs::UpdateGlobalConfigAccounts,
};

#[inline(never)]
pub fn process_update_global_config(
    accounts: &UpdateGlobalConfigAccounts,
    instruction: &UpdateGlobalConfigInstruction,
) -> LendingProgramResult {
    let mut global_config = accounts.global_config.load_mut();
    if instruction.accept_nomination {
        if global_config.can_upgrade_nomination(accounts.signer.key) {
            global_config.upgrade_nomination()?
        } else {
            return Err(LendingError::InvalidNomination.into());
        }
    }
    if !global_config.can_update_config(accounts.signer.key) {
        return Err(LendingAccountValidationError::InvalidProtocolAuthority.into());
    }
    if let Some(protocol_fee_share_in_bps) = instruction.protocol_fee_share_in_bps {
        global_config.update_protocol_fee_share_in_bps(protocol_fee_share_in_bps)?;
    }
    if let Some(fee_receiver) = instruction.fee_receiver {
        global_config.set_fee_receiver(fee_receiver);
    }
    if let Some(nominated_admin) = instruction.nominated_admin {
        global_config.set_nominated_admin(nominated_admin);
    }

    Ok(())
}
