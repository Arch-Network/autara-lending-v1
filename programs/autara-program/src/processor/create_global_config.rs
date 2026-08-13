use arch_program::{
    account::AccountInfo, program::invoke_signed_unchecked, pubkey::Pubkey, rent::minimum_rent,
    system_instruction,
};
use autara_lib::{
    ixs::CreateGlobalConfigInstruction, pda::global_config_seed_with_bump,
    state::global_config::GlobalConfig,
};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::{
    error::LendingProgramResult, ixs::CreateGlobalConfigAccounts, state::AutaraUninitializedAccount,
};

#[inline(never)]
pub fn process_create_global_config(
    create_global_config_accounts: &CreateGlobalConfigAccounts,
    data: &CreateGlobalConfigInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
) -> LendingProgramResult {
    let bump = [data.bump];
    let seed = global_config_seed_with_bump(&bump);
    invoke_signed_unchecked(
        &system_instruction::create_account(
            create_global_config_accounts.payer.key,
            create_global_config_accounts.global_config.key,
            minimum_rent(std::mem::size_of::<GlobalConfig>()),
            std::mem::size_of::<GlobalConfig>() as u64,
            program_id,
        ),
        accounts,
        &[&seed],
    )?;
    let global_config =
        ZeroCopyOwnedAccountMut::<AutaraUninitializedAccount<GlobalConfig>>::try_from(
            create_global_config_accounts.global_config,
        )?;
    let mut global_config_ref = global_config.load_mut();
    global_config_ref.initialize(
        data.admin,
        data.fee_receiver,
        data.protocol_fee_share_in_bps,
    );
    Ok(())
}
