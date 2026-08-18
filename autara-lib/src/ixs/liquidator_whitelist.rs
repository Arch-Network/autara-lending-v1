use arch_program::{account::AccountMeta, instruction::Instruction, pubkey::Pubkey};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::pda::find_liquidator_whitelist_entry_pda;

use super::types::AurataInstruction;

#[repr(C)]
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct AddWhitelistedLiquidatorInstruction {
    pub liquidator: Pubkey,
    pub bump: u8,
}

#[repr(C)]
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct RemoveWhitelistedLiquidatorInstruction {
    pub liquidator: Pubkey,
}

pub fn add_whitelisted_liquidator_ix(
    autara_program_id: Pubkey,
    market: Pubkey,
    curator: Pubkey,
    liquidator: Pubkey,
) -> (Pubkey, Instruction) {
    let (entry, bump) =
        find_liquidator_whitelist_entry_pda(&autara_program_id, &market, &liquidator);
    let accounts = vec![
        AccountMeta::new(market, false),
        AccountMeta::new(curator, true),
        AccountMeta::new(entry, false),
        AccountMeta::new_readonly(arch_program::system_program::SYSTEM_PROGRAM_ID, false),
        AccountMeta::new_readonly(autara_program_id, false),
    ];
    let mut data = Vec::new();
    AurataInstruction::AddWhitelistedLiquidator(AddWhitelistedLiquidatorInstruction {
        liquidator,
        bump,
    })
    .serialize(&mut data)
    .unwrap();
    (
        entry,
        Instruction {
            program_id: autara_program_id,
            accounts,
            data,
        },
    )
}

pub fn remove_whitelisted_liquidator_ix(
    autara_program_id: Pubkey,
    market: Pubkey,
    curator: Pubkey,
    liquidator: Pubkey,
) -> (Pubkey, Instruction) {
    let (entry, _) = find_liquidator_whitelist_entry_pda(&autara_program_id, &market, &liquidator);
    let accounts = vec![
        AccountMeta::new(market, false),
        AccountMeta::new_readonly(curator, true),
        AccountMeta::new(entry, false),
        AccountMeta::new_readonly(autara_program_id, false),
    ];
    let mut data = Vec::new();
    AurataInstruction::RemoveWhitelistedLiquidator(RemoveWhitelistedLiquidatorInstruction {
        liquidator,
    })
    .serialize(&mut data)
    .unwrap();
    (
        entry,
        Instruction {
            program_id: autara_program_id,
            accounts,
            data,
        },
    )
}
