use borsh::{BorshDeserialize, BorshSerialize};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum AurataInstructionTag {
    /// Create a new isolated market to lend some token A to be borrowed against some token B.
    /// It initialize the market config, the supply vault and the borrow vault.
    /// Curator is the only one who can update the market config later.
    /// The interest rate model is set at creation and cannot be changed later.
    /// Oracle config can be updated later, as well as other parameters of [MarketConfig](crate::state::market_config::MarketConfig).
    CreateMarket,
    /// Create a new supply position for a user in a specific market. Position cannot be transferred.
    /// The authority is the only one who can supply/withdraw to/from this position.
    CreateSupplyPosition,
    /// Supply APL token to a market, increasing the user's supply position.
    /// The user must have a supply position already created.
    /// By depositing some supply tokens, the user will receive a yield depending on the market's utilization rate.
    SupplyApl,
    /// Withdraw APL token from a market, decreasing the user's supply position.
    WithdrawSupply,
    /// Create a new borrow position for a user in a specific market. Position cannot be transferred.
    /// The authority is the only one who can borrow/repay from this position.
    CreateBorrowPosition,
    /// Deposit APL token as collateral to a borrow position.
    /// The user must have a borrow position already created.
    DepositAplCollateral,
    /// Withdraw APL token collateral from a borrow position.
    /// Withdrawing collateral will fail if the position becomes undercollateralized.
    WithdrawAplCollateral,
    /// Borrow APL token from a market, increasing the user's borrow position.
    /// Borrowing will fail if the position becomes undercollateralized.
    BorrowApl,
    /// Repay APL token to a market, decreasing the user's borrow position.
    RepayApl,
    /// Update the market config. Only the curator of the market can call this instruction.
    UpdateConfig,
    /// Liquidate a borrow position that is unhealthy.
    /// The liquidator repays part of the borrow and receives a portion of the collateral with a bonus.
    /// The liquidated position must remain healthy after the liquidation.
    Liquidate,
    /// Log an event. Cannot be called by an external user.
    Log,
    /// Create the global config account. Only callable once and defines the global admin.
    CreateGlobalConfig,
    /// Redeem the curator fees accumulated in the market. Only the curator can call this instruction.
    ReedeemCuratorFees,
    /// Redeem the protocol fees accumulated in the market. Only the program admin can call this instruction.
    ReedeemProtocolFees,
    /// Update the global config. Only the global admin can call this instruction.
    UpdateGlobalConfig,
    /// Borrow and deposit to collateral in a single atomic instruction with an optional callback.
    /// Usefull for leverage by swapping the borrowed asset to the collateral asset.
    BorrowDepositApl,
    /// Withdraw and repay in a single atomic instruction with an optional callback.
    /// Usefull for deleverage by swapping back the collateral asset to the borrowed asset.
    WithdrawRepayApl,
    /// Socialize losses across all suppliers when a undercollateralized borrow position hasnt been fully liquidated in time.
    /// Can only be called by the curator. The curator will collect the collateral and is expected to sell it somewhere to minimize the bad debt
    /// and then call donate supply to repay the amount recovered.
    SocializeLoss,
    /// Donate APL tokens to the supply vault of a market, increasing the total supply and the yield for all suppliers
    /// without receiving any supply shares in return.
    DonateSupply,
}

impl TryFrom<u8> for AurataInstructionTag {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AurataInstructionTag::CreateMarket),
            1 => Ok(AurataInstructionTag::CreateSupplyPosition),
            2 => Ok(AurataInstructionTag::SupplyApl),
            3 => Ok(AurataInstructionTag::WithdrawSupply),
            4 => Ok(AurataInstructionTag::CreateBorrowPosition),
            5 => Ok(AurataInstructionTag::DepositAplCollateral),
            6 => Ok(AurataInstructionTag::WithdrawAplCollateral),
            7 => Ok(AurataInstructionTag::BorrowApl),
            8 => Ok(AurataInstructionTag::RepayApl),
            9 => Ok(AurataInstructionTag::UpdateConfig),
            10 => Ok(AurataInstructionTag::Liquidate),
            11 => Ok(AurataInstructionTag::Log),
            12 => Ok(AurataInstructionTag::CreateGlobalConfig),
            13 => Ok(AurataInstructionTag::ReedeemCuratorFees),
            14 => Ok(AurataInstructionTag::ReedeemProtocolFees),
            15 => Ok(AurataInstructionTag::UpdateGlobalConfig),
            16 => Ok(AurataInstructionTag::BorrowDepositApl),
            17 => Ok(AurataInstructionTag::WithdrawRepayApl),
            18 => Ok(AurataInstructionTag::SocializeLoss),
            19 => Ok(AurataInstructionTag::DonateSupply),
            _ => Err(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AurataInstruction {
    CreateMarket(super::market::CreateMarketInstruction),
    CreateSupplyPosition(super::supply::CreateSupplyPositionInstruction),
    SupplyApl(super::supply::SupplyAplInstruction),
    WithdrawSupply(super::supply::WithdrawSupplyInstruction),
    CreateBorrowPosition(super::borrow::CreateBorrowPositionInstruction),
    BorrowApl(super::borrow::BorrowAplInstruction),
    DepositAplCollateral(super::borrow::DepositAplCollateralInstruction),
    WithdrawAplCollateral(super::borrow::WithdrawAplCollateralInstruction),
    RepayApl(super::borrow::RepayAplInstruction),
    UpdateConfig(super::market::UpdateConfigInstruction),
    Liquidate(super::liquidation::LiquidateInstruction),
    Log,
    CreateGlobalConfig(super::admin::CreateGlobalConfigInstruction),
    ReedeemCuratorFees,
    ReedeemProtocolFees,
    UpdateGlobalConfig(super::admin::UpdateGlobalConfigInstruction),
    BorrowDepositApl(super::borrow::BorrowDepositAplInstruction),
    WithdrawRepayApl(super::borrow::WithdrawRepayAplInstruction),
    SocializeLoss(super::liquidation::SocializeLossInstruction),
    DonateSupply(super::supply::DonateSupplyInstruction),
}

impl BorshSerialize for AurataInstruction {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        match self {
            AurataInstruction::CreateMarket(ix) => {
                AurataInstructionTag::CreateMarket.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::CreateSupplyPosition(ix) => {
                AurataInstructionTag::CreateSupplyPosition.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::SupplyApl(ix) => {
                AurataInstructionTag::SupplyApl.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::WithdrawSupply(ix) => {
                AurataInstructionTag::WithdrawSupply.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::BorrowApl(ix) => {
                AurataInstructionTag::BorrowApl.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::CreateBorrowPosition(ix) => {
                AurataInstructionTag::CreateBorrowPosition.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::DepositAplCollateral(ix) => {
                AurataInstructionTag::DepositAplCollateral.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::WithdrawAplCollateral(ix) => {
                AurataInstructionTag::WithdrawAplCollateral.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::RepayApl(ix) => {
                AurataInstructionTag::RepayApl.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::UpdateConfig(ix) => {
                AurataInstructionTag::UpdateConfig.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::Liquidate(ix) => {
                AurataInstructionTag::Liquidate.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::Log => AurataInstructionTag::Log.serialize(writer),
            AurataInstruction::CreateGlobalConfig(ix) => {
                AurataInstructionTag::CreateGlobalConfig.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::ReedeemCuratorFees => {
                AurataInstructionTag::ReedeemCuratorFees.serialize(writer)
            }
            AurataInstruction::ReedeemProtocolFees => {
                AurataInstructionTag::ReedeemProtocolFees.serialize(writer)
            }
            AurataInstruction::UpdateGlobalConfig(ix) => {
                AurataInstructionTag::UpdateGlobalConfig.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::BorrowDepositApl(ix) => {
                AurataInstructionTag::BorrowDepositApl.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::WithdrawRepayApl(ix) => {
                AurataInstructionTag::WithdrawRepayApl.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::SocializeLoss(ix) => {
                AurataInstructionTag::SocializeLoss.serialize(writer)?;
                ix.serialize(writer)
            }
            AurataInstruction::DonateSupply(ix) => {
                AurataInstructionTag::DonateSupply.serialize(writer)?;
                ix.serialize(writer)
            }
        }
    }
}

impl BorshDeserialize for AurataInstruction {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> Result<Self, std::io::Error> {
        let tag = AurataInstructionTag::deserialize_reader(reader)?;
        match tag {
            AurataInstructionTag::CreateMarket => Ok(AurataInstruction::CreateMarket(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::CreateSupplyPosition => Ok(
                AurataInstruction::CreateSupplyPosition(<_>::deserialize_reader(reader)?),
            ),
            AurataInstructionTag::SupplyApl => Ok(AurataInstruction::SupplyApl(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::WithdrawSupply => Ok(AurataInstruction::WithdrawSupply(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::BorrowApl => Ok(AurataInstruction::BorrowApl(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::CreateBorrowPosition => Ok(
                AurataInstruction::CreateBorrowPosition(<_>::deserialize_reader(reader)?),
            ),
            AurataInstructionTag::DepositAplCollateral => Ok(
                AurataInstruction::DepositAplCollateral(<_>::deserialize_reader(reader)?),
            ),
            AurataInstructionTag::WithdrawAplCollateral => Ok(
                AurataInstruction::WithdrawAplCollateral(<_>::deserialize_reader(reader)?),
            ),
            AurataInstructionTag::RepayApl => Ok(AurataInstruction::RepayApl(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::UpdateConfig => Ok(AurataInstruction::UpdateConfig(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::Liquidate => Ok(AurataInstruction::Liquidate(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::Log => Ok(AurataInstruction::Log),
            AurataInstructionTag::CreateGlobalConfig => Ok(AurataInstruction::CreateGlobalConfig(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::ReedeemCuratorFees => Ok(AurataInstruction::ReedeemCuratorFees),
            AurataInstructionTag::ReedeemProtocolFees => Ok(AurataInstruction::ReedeemProtocolFees),
            AurataInstructionTag::UpdateGlobalConfig => Ok(AurataInstruction::UpdateGlobalConfig(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::BorrowDepositApl => Ok(AurataInstruction::BorrowDepositApl(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::WithdrawRepayApl => Ok(AurataInstruction::WithdrawRepayApl(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::SocializeLoss => Ok(AurataInstruction::SocializeLoss(
                <_>::deserialize_reader(reader)?,
            )),
            AurataInstructionTag::DonateSupply => Ok(AurataInstruction::DonateSupply(
                <_>::deserialize_reader(reader)?,
            )),
        }
    }
}
