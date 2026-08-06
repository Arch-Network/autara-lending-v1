use arch_program::pubkey::Pubkey;
use borsh::{BorshDeserialize, BorshSerialize};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::{
    ixs::AurataInstructionTag,
    oracle::oracle_price::OracleRate,
    state::{borrow_position::BorrowPositionHealth, supply_vault::SupplyVaultSummary},
};

#[repr(u8)]
#[derive(
    Clone, Copy, Debug, PartialEq, BorshSerialize, BorshDeserialize, IntoPrimitive, TryFromPrimitive,
)]
pub enum AurataEventTag {
    Liquidate,
    Supply,
    Withdraw,
    DepositCollateral,
    WithdrawCollateral,
    Borrow,
    Repay,
    RedeemProtocolFees,
    RedeemCuratorFees,
    DepositAndBorrow,
    WithdrawAndRepay,
    SocializeLoss,
    Donation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct LiquidateEvent {
    pub market: Pubkey,
    pub liquidator: Pubkey,
    pub liquidatee_position: Pubkey,
    pub supply_mint: Pubkey,
    pub collateral_mint: Pubkey,
    pub health_before_liquidation: BorrowPositionHealth,
    pub health_after_liquidation: BorrowPositionHealth,
    pub supply_repaid: u64,
    pub collateral_liquidated: u64,
    pub liquidator_fee: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct SingleMarketTransactionEvent {
    pub market: Pubkey,
    pub user: Pubkey,
    pub position: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
    pub supply_vault_summary: SupplyVaultSummary,
    pub collateral_vault_atoms: u64,
    pub supply_oracle_rate: OracleRate,
    pub collateral_oracle_rate: OracleRate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct DoubleMarketTransactionEvent {
    pub market: Pubkey,
    pub user: Pubkey,
    pub position: Pubkey,
    pub mint_in: Pubkey,
    pub amount_in: u64,
    pub mint_out: Pubkey,
    pub amount_out: u64,
    pub supply_vault_summary: SupplyVaultSummary,
    pub collateral_vault_atoms: u64,
    pub supply_oracle_rate: OracleRate,
    pub collateral_oracle_rate: OracleRate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct ReedeemFeeEvent {
    pub market: Pubkey,
    pub fee_receiver: Pubkey,
    pub fee_amount: u64,
    pub mint: Pubkey,
    pub supply_vault_snapshot: SupplyVaultSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct SocializeLossEvent {
    pub market: Pubkey,
    pub position: Pubkey,
    pub debt_socialized: u64,
    pub collateral_liquidated: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct DonateSupplyEvent {
    pub market: Pubkey,
    pub donor: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(tag = "type", content = "event")
)]
pub enum AutaraEvent {
    Liquidate(LiquidateEvent),
    Supply(SingleMarketTransactionEvent),
    Withdraw(SingleMarketTransactionEvent),
    DepositCollateral(SingleMarketTransactionEvent),
    WithdrawCollateral(SingleMarketTransactionEvent),
    BorrowAndDeposit(DoubleMarketTransactionEvent),
    WithdrawAndRepay(DoubleMarketTransactionEvent),
    Borrow(SingleMarketTransactionEvent),
    Repay(SingleMarketTransactionEvent),
    ReedeemProtocolFees(ReedeemFeeEvent),
    ReedeemCuratorFees(ReedeemFeeEvent),
    SocializeLoss(SocializeLossEvent),
    DonateSupply(DonateSupplyEvent),
}

impl AutaraEvent {
    pub fn from_bytes(bytes: &[u8]) -> Option<AutaraEvent> {
        let mut cursor = &mut &bytes[..];
        let tag = AurataInstructionTag::deserialize(&mut cursor).ok()?;
        if tag != AurataInstructionTag::Log {
            return None;
        }
        AutaraEvent::deserialize(cursor).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[cfg_attr(
    feature = "client",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct AutaraEvents {
    pub events: Vec<AutaraEvent>,
}

#[cfg(feature = "client")]
pub mod client {
    use super::*;
    use arch_sdk::ProcessedTransaction;

    impl AutaraEvents {
        pub fn from_processed_tx(
            tx: &ProcessedTransaction,
            validate_program_id: impl Fn(&Pubkey) -> bool,
        ) -> Self {
            AutaraEvents {
                events: get_ix_data_with_program_ids(tx)
                    .filter_map(|ix| {
                        if validate_program_id(ix.0) {
                            AutaraEvent::from_bytes(ix.1)
                        } else {
                            None
                        }
                    })
                    .collect(),
            }
        }
    }

    pub fn get_ix_data_with_program_ids(
        tx: &ProcessedTransaction,
    ) -> impl Iterator<Item = (&arch_program::pubkey::Pubkey, &[u8])> {
        tx.inner_instructions_list
            .iter()
            .flat_map(|ixs| ixs.iter())
            .filter_map(|ix| {
                Some((
                    tx.runtime_transaction
                        .message
                        .get_account_key(ix.instruction.program_id_index as _)?,
                    ix.instruction.data.as_slice(),
                ))
            })
    }
}

impl BorshSerialize for AutaraEvent {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        match self {
            AutaraEvent::Liquidate(event) => {
                AurataEventTag::Liquidate.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::Supply(event) => {
                AurataEventTag::Supply.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::Withdraw(event) => {
                AurataEventTag::Withdraw.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::DepositCollateral(event) => {
                AurataEventTag::DepositCollateral.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::WithdrawCollateral(event) => {
                AurataEventTag::WithdrawCollateral.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::Borrow(event) => {
                AurataEventTag::Borrow.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::Repay(event) => {
                AurataEventTag::Repay.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::ReedeemProtocolFees(event) => {
                AurataEventTag::RedeemProtocolFees.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::ReedeemCuratorFees(event) => {
                AurataEventTag::RedeemCuratorFees.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::BorrowAndDeposit(event) => {
                AurataEventTag::DepositAndBorrow.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::WithdrawAndRepay(event) => {
                AurataEventTag::WithdrawAndRepay.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::SocializeLoss(event) => {
                AurataEventTag::SocializeLoss.serialize(writer)?;
                event.serialize(writer)
            }
            AutaraEvent::DonateSupply(event) => {
                AurataEventTag::Donation.serialize(writer)?;
                event.serialize(writer)
            }
        }
    }
}

impl BorshDeserialize for AutaraEvent {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> Result<Self, std::io::Error> {
        let tag = AurataEventTag::deserialize_reader(reader)?;
        match tag {
            AurataEventTag::Liquidate => {
                Ok(AutaraEvent::Liquidate(<_>::deserialize_reader(reader)?))
            }
            AurataEventTag::Supply => Ok(AutaraEvent::Supply(<_>::deserialize_reader(reader)?)),
            AurataEventTag::Withdraw => Ok(AutaraEvent::Withdraw(<_>::deserialize_reader(reader)?)),
            AurataEventTag::DepositCollateral => Ok(AutaraEvent::DepositCollateral(
                <_>::deserialize_reader(reader)?,
            )),
            AurataEventTag::WithdrawCollateral => Ok(AutaraEvent::WithdrawCollateral(
                <_>::deserialize_reader(reader)?,
            )),
            AurataEventTag::Borrow => Ok(AutaraEvent::Borrow(<_>::deserialize_reader(reader)?)),
            AurataEventTag::Repay => Ok(AutaraEvent::Repay(<_>::deserialize_reader(reader)?)),
            AurataEventTag::RedeemProtocolFees => Ok(AutaraEvent::ReedeemProtocolFees(
                <_>::deserialize_reader(reader)?,
            )),
            AurataEventTag::RedeemCuratorFees => Ok(AutaraEvent::ReedeemCuratorFees(
                <_>::deserialize_reader(reader)?,
            )),
            AurataEventTag::DepositAndBorrow => Ok(AutaraEvent::BorrowAndDeposit(
                <_>::deserialize_reader(reader)?,
            )),
            AurataEventTag::WithdrawAndRepay => Ok(AutaraEvent::WithdrawAndRepay(
                <_>::deserialize_reader(reader)?,
            )),
            AurataEventTag::SocializeLoss => {
                Ok(AutaraEvent::SocializeLoss(<_>::deserialize_reader(reader)?))
            }
            AurataEventTag::Donation => {
                Ok(AutaraEvent::DonateSupply(<_>::deserialize_reader(reader)?))
            }
        }
    }
}
