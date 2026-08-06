use arch_program::{account::AccountInfo, msg, program_error::ProgramError, pubkey::Pubkey};
use autara_lib::ixs::*;
use borsh::BorshDeserialize;

use crate::{
    error::LendingProgramResult,
    ixs::{
        redeem_curator_fees::RedeemCuratorFeesAccounts,
        redeem_protocol_fees::RedeemProtocolFeesAccounts, *,
    },
    processor::{
        borrow_apl::process_borrow_apl, borrow_deposit_apl::process_borrow_deposit_apl,
        create_borrow_position::process_create_borrow_position,
        create_global_config::process_create_global_config, create_market::process_create_market,
        create_supply_position::process_create_supply_position,
        deposit_apl_collateral::process_deposit_apl_collateral,
        donate_supply::process_donate_supply, liquidate::process_liquidate,
        redeem_curator_fees::process_redeem_curator_fees,
        redeem_protocol_fees::process_redeem_protocol_fees, repay_apl::process_repay_apl,
        socialize_loss::process_socialize_loss, supply_apl::process_supply_apl,
        update_config::process_update_config, update_global_config::process_update_global_config,
        withdraw_apl_collateral::process_withdraw_apl_collateral,
        withdraw_repay_apl::process_withdraw_repay_apl, withdraw_supply::process_withdraw_supply,
    },
};

pub mod error;
pub mod ixs;
pub mod processor;
pub mod state;
pub mod utils;

pub const fn id() -> Pubkey {
    Pubkey(hex_literal::hex!(
        "19da5f9b75103d4384b5b78e4b7535198ab0c788578df142db5992a62e30bad0"
    ))
}

#[cfg(feature = "entrypoint")]
arch_program::entrypoint!(process_instruction);

pub fn process_instruction<'a>(
    program_id: &Pubkey,
    accounts: &'a [AccountInfo<'a>],
    instruction_data: &[u8],
) -> Result<(), ProgramError> {
    autara_process_instruction(program_id, accounts, instruction_data).map_err(Into::into)
}

pub fn autara_process_instruction<'a>(
    program_id: &Pubkey,
    accounts: &'a [AccountInfo<'a>],
    instruction_data: &[u8],
) -> LendingProgramResult {
    let mut accounts_iter = &mut accounts.iter();
    let clock = utils::clock();
    let ix = <Box<AurataInstruction>>::deserialize(&mut &instruction_data[..])
        .map_err(|_| ProgramError::InvalidInstructionData)?;
    match &*ix {
        AurataInstruction::CreateMarket(data) => {
            msg!("Processing CreateMarket instruction");
            let create_market_accounts = CreateMarketAccounts::from_accounts(&mut accounts_iter)?;
            process_create_market(&create_market_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::CreateSupplyPosition(data) => {
            msg!("Processing CreateSupplyPosition instruction");
            let create_supply_position_accounts =
                CreateSupplyPositionAccounts::from_accounts(&mut accounts_iter)?;
            process_create_supply_position(
                &create_supply_position_accounts,
                data,
                accounts,
                program_id,
            )
        }
        AurataInstruction::SupplyApl(data) => {
            msg!("Processing SupplyApl instruction");
            let supply_apl_accounts = SupplyAplAccounts::from_accounts(&mut accounts_iter)?;
            process_supply_apl(&supply_apl_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::WithdrawSupply(data) => {
            msg!("Processing WithdrawSupply instruction");
            let withdraw_supply_accounts =
                WithdrawSupplyAccounts::from_accounts(&mut accounts_iter)?;
            process_withdraw_supply(
                &withdraw_supply_accounts,
                data,
                accounts,
                program_id,
                &clock,
            )
        }
        AurataInstruction::CreateBorrowPosition(data) => {
            msg!("Processing CreateBorrowPosition instruction");
            let create_borrow_position_accounts =
                CreateBorrowPositionAccounts::from_accounts(&mut accounts_iter)?;
            process_create_borrow_position(
                &create_borrow_position_accounts,
                data,
                accounts,
                program_id,
            )
        }
        AurataInstruction::BorrowApl(data) => {
            msg!("Processing BorrowApl instruction");
            let borrow_apl_accounts = BorrowAplAccounts::from_accounts(&mut accounts_iter)?;
            process_borrow_apl(&borrow_apl_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::DepositAplCollateral(data) => {
            msg!("Processing DepositAplCollateral instruction");
            let deposit_apl_collateral_accounts =
                DepositAplCollateralAccounts::from_accounts(&mut accounts_iter)?;
            process_deposit_apl_collateral(
                &deposit_apl_collateral_accounts,
                data,
                accounts,
                program_id,
                &clock,
            )
        }
        AurataInstruction::RepayApl(data) => {
            msg!("Processing RepayAplCollateral instruction");
            let repay_apl_accounts = RepayAplAccounts::from_accounts(&mut accounts_iter)?;
            process_repay_apl(&repay_apl_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::UpdateConfig(data) => {
            msg!("Processing UpdateConfig instruction");
            let update_config_accounts = UpdateConfigAccounts::from_accounts(&mut accounts_iter)?;
            process_update_config(&update_config_accounts, data, &clock)
        }
        AurataInstruction::WithdrawAplCollateral(data) => {
            msg!("Processing WithdrawAplCollateral instruction");
            let withdraw_apl_collateral_accounts =
                WithdrawAplCollateralAccounts::from_accounts(&mut accounts_iter)?;
            process_withdraw_apl_collateral(
                &withdraw_apl_collateral_accounts,
                data,
                accounts,
                program_id,
                &clock,
            )
        }
        AurataInstruction::Liquidate(data) => {
            msg!("Processing Liquidate instruction");
            let liquidate_accounts = LiquidateAccounts::from_accounts(&mut accounts_iter)?;
            process_liquidate(&liquidate_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::CreateGlobalConfig(data) => {
            msg!("Processing CreateGlobalConfig instruction");
            let create_global_config_accounts =
                CreateGlobalConfigAccounts::from_accounts(&mut accounts_iter)?;
            process_create_global_config(&create_global_config_accounts, data, accounts, program_id)
        }
        AurataInstruction::ReedeemCuratorFees => {
            msg!("Processing ReedeemCuratorFees instruction");
            let redeem_curator_fees_accounts =
                RedeemCuratorFeesAccounts::from_accounts(&mut accounts_iter)?;
            process_redeem_curator_fees(&redeem_curator_fees_accounts, accounts, program_id, &clock)
        }
        AurataInstruction::ReedeemProtocolFees => {
            msg!("Processing ReedeemProtocolFees instruction");
            let redeem_protocol_fees_accounts =
                RedeemProtocolFeesAccounts::from_accounts(&mut accounts_iter)?;
            process_redeem_protocol_fees(
                &redeem_protocol_fees_accounts,
                accounts,
                program_id,
                &clock,
            )
        }
        AurataInstruction::UpdateGlobalConfig(data) => {
            msg!("Processing UpdateGlobalConfig instruction");
            let update_global_config_accounts =
                UpdateGlobalConfigAccounts::from_accounts(&mut accounts_iter)?;
            process_update_global_config(&update_global_config_accounts, data)
        }
        AurataInstruction::BorrowDepositApl(data) => {
            msg!("Processing BorrowDepositApl instruction");
            let borrow_deposit_apl_accounts =
                BorrowDepositAplAccounts::from_accounts(&mut accounts_iter)?;
            process_borrow_deposit_apl(
                &borrow_deposit_apl_accounts,
                data,
                accounts,
                program_id,
                &clock,
            )
        }
        AurataInstruction::WithdrawRepayApl(data) => {
            msg!("Processing WithdrawRepayApl instruction");
            let withdraw_repay_apl_accounts =
                WithdrawRepayAplAccounts::from_accounts(&mut accounts_iter)?;
            process_withdraw_repay_apl(
                &withdraw_repay_apl_accounts,
                data,
                accounts,
                program_id,
                &clock,
            )
        }
        AurataInstruction::SocializeLoss(data) => {
            msg!("Processing SocializeLoss instruction");
            let socialize_loss_accounts = SocializeLossAccounts::from_accounts(&mut accounts_iter)?;
            process_socialize_loss(&socialize_loss_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::DonateSupply(data) => {
            msg!("Processing DonateSupply instruction");
            let donate_supply_accounts = DonateSupplyAccounts::from_accounts(&mut accounts_iter)?;
            process_donate_supply(&donate_supply_accounts, data, accounts, program_id, &clock)
        }
        AurataInstruction::Log => {
            let _check_accounts = LogAccounts::from_accounts(&mut accounts_iter)?;
            Ok(())
        }
    }
}
