use arch_program::{
    account::AccountInfo, program::invoke_signed_unchecked, pubkey::Pubkey, rent::minimum_rent,
    system_instruction,
};
use autara_lib::{
    ixs::supply::CreateSupplyPositionInstruction, pda::supply_position_seed_with_bump,
    state::supply_position::SupplyPosition,
};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::{
    error::LendingProgramResult, ixs::CreateSupplyPositionAccounts,
    state::AutaraUninitializedAccount,
};

#[inline(never)]
pub fn process_create_supply_position(
    create_supply_position_accounts: &CreateSupplyPositionAccounts,
    data: &CreateSupplyPositionInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
) -> LendingProgramResult {
    let bump = [data.bump];
    let seed = supply_position_seed_with_bump(
        create_supply_position_accounts.market.key(),
        create_supply_position_accounts.authority.key,
        &bump,
    );
    invoke_signed_unchecked(
        &system_instruction::create_account(
            create_supply_position_accounts.payer.key,
            create_supply_position_accounts.supply_position.key,
            minimum_rent(std::mem::size_of::<SupplyPosition>()),
            std::mem::size_of::<SupplyPosition>() as u64,
            program_id,
        ),
        accounts,
        &[&seed],
    )?;
    let position = ZeroCopyOwnedAccountMut::<AutaraUninitializedAccount<SupplyPosition>>::try_from(
        create_supply_position_accounts.supply_position,
    )?;
    let mut position = position.load_mut();
    position.initialize(
        *create_supply_position_accounts.authority.key,
        *create_supply_position_accounts.market.key(),
    );
    Ok(())
}
