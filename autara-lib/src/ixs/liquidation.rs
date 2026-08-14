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
    liquidator_whitelist_entry: Option<Pubkey>,
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
    ];
    if let Some(entry) = liquidator_whitelist_entry {
        accounts.push(AccountMeta::new_readonly(entry, false));
    }
    accounts.push(AccountMeta::new_readonly(autara_program_id, false));
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

#[repr(C)]
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct BeginCapitalSweepInstruction {}

pub fn begin_capital_sweep_ix(
    autara_program_id: Pubkey,
    market: Pubkey,
    borrow_position: Pubkey,
    curator: Pubkey,
    curator_collateral_ata: Pubkey,
    market_collateral_vault: Pubkey,
    supply_oracle: Pubkey,
    collateral_oracle: Pubkey,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new(market, false),
        AccountMeta::new(borrow_position, false),
        AccountMeta::new_readonly(curator, true),
        AccountMeta::new(curator_collateral_ata, false),
        AccountMeta::new(market_collateral_vault, false),
        AccountMeta::new_readonly(apl_token::id(), false),
        AccountMeta::new_readonly(supply_oracle, false),
        AccountMeta::new_readonly(collateral_oracle, false),
        AccountMeta::new_readonly(autara_program_id, false),
    ];
    let mut data = Vec::new();
    AurataInstruction::BeginCapitalSweep(BeginCapitalSweepInstruction {})
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
pub struct SettleCapitalSweepInstruction {
    pub max_borrowed_atoms_to_repay: u64,
    pub max_collateral_atoms_to_return: u64,
}

pub fn settle_capital_sweep_ix(
    autara_program_id: Pubkey,
    market: Pubkey,
    borrow_position: Pubkey,
    curator: Pubkey,
    curator_supply_ata: Pubkey,
    curator_collateral_ata: Pubkey,
    market_supply_vault: Pubkey,
    market_collateral_vault: Pubkey,
    supply_oracle: Pubkey,
    collateral_oracle: Pubkey,
    max_borrowed_atoms_to_repay: u64,
    max_collateral_atoms_to_return: u64,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new(market, false),
        AccountMeta::new(borrow_position, false),
        AccountMeta::new_readonly(curator, true),
        AccountMeta::new(curator_supply_ata, false),
        AccountMeta::new(curator_collateral_ata, false),
        AccountMeta::new(market_supply_vault, false),
        AccountMeta::new(market_collateral_vault, false),
        AccountMeta::new_readonly(apl_token::id(), false),
        AccountMeta::new_readonly(supply_oracle, false),
        AccountMeta::new_readonly(collateral_oracle, false),
        AccountMeta::new_readonly(autara_program_id, false),
    ];
    let mut data = Vec::new();
    AurataInstruction::SettleCapitalSweep(SettleCapitalSweepInstruction {
        max_borrowed_atoms_to_repay,
        max_collateral_atoms_to_return,
    })
    .serialize(&mut data)
    .unwrap();
    Instruction {
        program_id: autara_program_id,
        accounts,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liquidate_builder_places_whitelist_proof_before_program_and_callback() {
        let program = Pubkey::new_unique();
        let callback_program = Pubkey::new_unique();
        let whitelist_entry = Pubkey::new_unique();
        let callback = Instruction {
            program_id: callback_program,
            accounts: vec![],
            data: vec![],
        };

        let restricted = liquidate_ix(
            program,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            u64::MAX,
            0,
            Some(whitelist_entry),
            Some(callback),
        );
        assert_eq!(restricted.accounts[10].pubkey, whitelist_entry);
        assert_eq!(restricted.accounts[11].pubkey, program);
        assert_eq!(restricted.accounts[12].pubkey, callback_program);

        let permissionless = liquidate_ix(
            program,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            u64::MAX,
            0,
            None,
            None,
        );
        assert_eq!(permissionless.accounts[10].pubkey, program);
    }

    #[test]
    fn liquidator_whitelist_builders_use_expected_pda_and_accounts() {
        let program = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let curator = Pubkey::new_unique();
        let liquidator = Pubkey::new_unique();

        let (entry, add) =
            crate::ixs::add_whitelisted_liquidator_ix(program, market, curator, liquidator);
        assert_eq!(
            entry,
            crate::pda::find_liquidator_whitelist_entry_pda(&program, &market, &liquidator).0
        );
        assert_eq!(add.accounts[0].pubkey, market);
        assert_eq!(add.accounts[1].pubkey, curator);
        assert!(add.accounts[1].is_signer);
        assert!(add.accounts[1].is_writable);
        assert_eq!(add.accounts[2].pubkey, entry);
        assert_eq!(
            add.accounts[3].pubkey,
            arch_program::system_program::SYSTEM_PROGRAM_ID
        );
        assert_eq!(add.accounts[4].pubkey, program);

        let (removed_entry, remove) =
            crate::ixs::remove_whitelisted_liquidator_ix(program, market, curator, liquidator);
        assert_eq!(removed_entry, entry);
        assert_eq!(remove.accounts[0].pubkey, market);
        assert_eq!(remove.accounts[1].pubkey, curator);
        assert!(remove.accounts[1].is_signer);
        assert_eq!(remove.accounts[2].pubkey, entry);
        assert_eq!(remove.accounts[3].pubkey, program);
    }

    #[test]
    fn capital_sweep_builders_encode_expected_accounts_and_data() {
        let program = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let position = Pubkey::new_unique();
        let curator = Pubkey::new_unique();
        let curator_supply = Pubkey::new_unique();
        let curator_collateral = Pubkey::new_unique();
        let market_supply = Pubkey::new_unique();
        let market_collateral = Pubkey::new_unique();
        let supply_oracle = Pubkey::new_unique();
        let collateral_oracle = Pubkey::new_unique();

        let begin = begin_capital_sweep_ix(
            program,
            market,
            position,
            curator,
            curator_collateral,
            market_collateral,
            supply_oracle,
            collateral_oracle,
        );
        assert_eq!(begin.accounts.len(), 9);
        assert_eq!(begin.accounts[0].pubkey, market);
        assert_eq!(begin.accounts[1].pubkey, position);
        assert_eq!(begin.accounts[2].pubkey, curator);
        assert!(begin.accounts[2].is_signer);
        assert_eq!(begin.accounts[3].pubkey, curator_collateral);
        assert_eq!(begin.accounts[4].pubkey, market_collateral);
        assert_eq!(
            AurataInstruction::try_from_slice(&begin.data).unwrap(),
            AurataInstruction::BeginCapitalSweep(BeginCapitalSweepInstruction {})
        );

        let settle = settle_capital_sweep_ix(
            program,
            market,
            position,
            curator,
            curator_supply,
            curator_collateral,
            market_supply,
            market_collateral,
            supply_oracle,
            collateral_oracle,
            123,
            456,
        );
        assert_eq!(settle.accounts.len(), 11);
        assert_eq!(settle.accounts[2].pubkey, curator);
        assert!(settle.accounts[2].is_signer);
        assert_eq!(settle.accounts[3].pubkey, curator_supply);
        assert_eq!(settle.accounts[4].pubkey, curator_collateral);
        assert_eq!(settle.accounts[5].pubkey, market_supply);
        assert_eq!(settle.accounts[6].pubkey, market_collateral);
        assert_eq!(
            AurataInstruction::try_from_slice(&settle.data).unwrap(),
            AurataInstruction::SettleCapitalSweep(SettleCapitalSweepInstruction {
                max_borrowed_atoms_to_repay: 123,
                max_collateral_atoms_to_return: 456,
            })
        );
    }
}
