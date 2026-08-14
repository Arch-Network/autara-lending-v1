use arch_program::{
    account::AccountInfo, program::invoke_signed_unchecked, pubkey::Pubkey, rent::minimum_rent,
    system_instruction,
};
use autara_lib::{
    ixs::borrow::CreateBorrowPositionInstruction, pda::borrow_position_seed_with_bump,
    state::borrow_position::BorrowPosition,
};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::{
    error::LendingProgramResult, ixs::CreateBorrowPositionAccounts,
    state::AutaraUninitializedAccount,
};

#[inline(never)]
pub fn process_create_borrow_position(
    create_borrow_position_accounts: &CreateBorrowPositionAccounts,
    data: &CreateBorrowPositionInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
) -> LendingProgramResult {
    let bump = [data.bump];
    let seed = borrow_position_seed_with_bump(
        create_borrow_position_accounts.market.key(),
        create_borrow_position_accounts.authority.key,
        &bump,
    );
    invoke_signed_unchecked(
        &system_instruction::create_account(
            create_borrow_position_accounts.payer.key,
            create_borrow_position_accounts.borrow_position.key,
            minimum_rent(std::mem::size_of::<BorrowPosition>()),
            std::mem::size_of::<BorrowPosition>() as u64,
            program_id,
        ),
        accounts,
        &[&seed],
    )?;
    let position = ZeroCopyOwnedAccountMut::<AutaraUninitializedAccount<BorrowPosition>>::try_from(
        create_borrow_position_accounts.borrow_position,
    )?;
    let mut position = position.load_mut();
    position.initialize(
        *create_borrow_position_accounts.authority.key,
        *create_borrow_position_accounts.market.key(),
    );
    Ok(())
}
