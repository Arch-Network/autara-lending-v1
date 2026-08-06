use arch_program::{account::AccountMeta, instruction::Instruction, pubkey::Pubkey};
use borsh::{BorshDeserialize, BorshSerialize};

use super::types::AurataInstruction;

#[repr(C)]
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct LiquidateInstruction {
    pub max_borrowed_atoms_to_repay: u64,
    pub min_collateral_atoms_to_receive: u64,
    /// Optional callback instruction to be executed
    /// after receiving collateral and before repaying debt
    /// Usefull for atomic liquidation
    #[cfg_attr(feature = "client", serde(default))]
    pub ix_callback: Option<Instruction>,
}

pub fn liquidate_ix(
    autara_program_id: Pubkey,
    market: Pubkey,
    borrow_position: Pubkey,
    liquidator: Pubkey,
    liquidator_supply_ata: Pubkey,
    liquidator_collateral_ata: Pubkey,
    market_supply_vault: Pubkey,
    market_collateral_vault: Pubkey,
    supply_oracle: Pubkey,
    collateral_oracle: Pubkey,
    max_borrowed_atoms_to_repay: u64,
    min_collateral_atoms_to_receive: u64,
    ix_callback: Option<Instruction>,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(market, false),
        AccountMeta::new(borrow_position, false),
        AccountMeta::new_readonly(liquidator, true),
        AccountMeta::new(liquidator_supply_ata, false),
        AccountMeta::new(liquidator_collateral_ata, false),
        AccountMeta::new(market_supply_vault, false),
        AccountMeta::new(market_collateral_vault, false),
        AccountMeta::new_readonly(apl_token::id(), false),
        AccountMeta::new_readonly(supply_oracle, false),
        AccountMeta::new_readonly(collateral_oracle, false),
        AccountMeta::new_readonly(autara_program_id, false),
    ];
    if let Some(callback) = &ix_callback {
        accounts.push(AccountMeta::new_readonly(callback.program_id, false));
        accounts.extend(callback.accounts.iter().cloned());
    }
    let mut data = Vec::new();
    AurataInstruction::Liquidate(LiquidateInstruction {
        max_borrowed_atoms_to_repay,
        min_collateral_atoms_to_receive,
        ix_callback,
    })
    .serialize(&mut data)
    .unwrap();
    Instruction {
        program_id: autara_program_id,
        accounts,
        data,
    }
}

#[repr(C)]
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct SocializeLossInstruction {}

pub fn socialize_loss_ix(
    autara_program_id: Pubkey,
    market: Pubkey,
    borrow_position: Pubkey,
    curator: Pubkey,
    receiver_collateral_ata: Pubkey,
    market_collateral_vault: Pubkey,
    supply_oracle: Pubkey,
    collateral_oracle: Pubkey,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new(market, false),
        AccountMeta::new(borrow_position, false),
        AccountMeta::new_readonly(curator, true),
        AccountMeta::new(receiver_collateral_ata, false),
        AccountMeta::new(market_collateral_vault, false),
        AccountMeta::new_readonly(apl_token::id(), false),
        AccountMeta::new_readonly(supply_oracle, false),
        AccountMeta::new_readonly(collateral_oracle, false),
        AccountMeta::new_readonly(autara_program_id, false),
    ];
    let mut data = Vec::new();
    AurataInstruction::SocializeLoss(SocializeLossInstruction {})
        .serialize(&mut data)
        .unwrap();
    Instruction {
        program_id: autara_program_id,
        accounts,
        data,
    }
}
