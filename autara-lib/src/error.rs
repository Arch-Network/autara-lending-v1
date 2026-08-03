use std::ops::Deref;

use num_enum::{IntoPrimitive, TryFromPrimitive};

pub type LendingResult<T = ()> = Result<T, ErrorWithContext<LendingError>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("error = {error}, msg = {msg:?}, stack = {stack:?}")]
pub struct ErrorWithContext<T: std::fmt::Display> {
    pub error: T,
    pub msg: Vec<DisplayCow>,
    pub stack: Vec<DisplayLocation>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DisplayLocation(pub &'static std::panic::Location<'static>);

impl std::fmt::Debug for DisplayLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DisplayCow(pub std::borrow::Cow<'static, str>);

impl std::fmt::Debug for DisplayCow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.as_ref())
    }
}

impl<T: std::fmt::Display> ErrorWithContext<T> {
    pub fn new(error: T, location: &'static std::panic::Location<'static>) -> Self {
        let mut context = Vec::with_capacity(4);
        context.push(DisplayLocation(location));
        ErrorWithContext {
            error,
            stack: context,
            msg: Vec::with_capacity(2),
        }
    }
}

impl<T: std::fmt::Display> Deref for ErrorWithContext<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.error
    }
}

impl PartialEq<LendingError> for ErrorWithContext<LendingError> {
    fn eq(&self, other: &LendingError) -> bool {
        self.error == *other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive, thiserror::Error)]
#[repr(u8)]
pub enum LendingError {
    #[error("Math overflow")]
    MathOverflow,
    #[error("Addition overflow")]
    AdditionOverflow,
    #[error("Subtraction overflow")]
    SubtractionOverflow,
    #[error("Multiplication overflow")]
    MultiplicationOverflow,
    #[error("Division overflow")]
    DivisionOverflow,
    #[error("Division by zero")]
    DivisionByZero,
    #[error("Numeric cast overflow")]
    CastOverflow,
    #[error("Borrow would exceed the market max loan-to-value (LTV)")]
    MaxLtvReached,
    #[error("Borrow would exceed the market max utilisation rate")]
    MaxUtilisationRateReached,
    #[error("Position does not belong to this market")]
    InvalidMarketForPosition,
    #[error("Position is healthy and cannot be liquidated")]
    PositionIsHealthy,
    #[error("Supply would exceed the market max supply")]
    MaxSupplyReached,
    #[error("Invalid LTV configuration")]
    InvalidLtvConfig,
    #[error("Invalid interest rate curve")]
    InvalidCurve,
    #[error("Invalid exponential function argument")]
    InvalidExpArg,
    #[error("Invalid max utilisation rate")]
    InvalidMaxUtilisationRate,
    #[error("Liquidation must reduce the position loan-to-value (LTV)")]
    InvalidLiquidationLtvShouldDecrease,
    #[error("Invalid Pyth oracle account")]
    InvalidPythOracleAccount,
    #[error("Invalid Chaos Labs oracle account")]
    InvalidChaosOracleAccount,
    #[error("Invalid oracle feed id")]
    InvalidOracleFeedId,
    #[error("Failed to load account")]
    FailedToLoadAccount,
    #[error("Withdrawal exceeds available market reserves")]
    WithdrawalExceedsReserves,
    #[error("Withdrawal exceeds the amount deposited")]
    WithdrawalExceedsDeposited,
    #[error("Repay amount exceeds the outstanding borrow")]
    RepayExceedsBorrowed,
    #[error("Oracle price is too old")]
    OracleRateTooOld,
    #[error("Oracle price confidence is too low relative to the price")]
    OracleRateRelativeConfidenceTooLow,
    #[error("Oracle price is negative")]
    NegativeOracleRate,
    #[error("Oracle price is zero or null")]
    OracleRateIsNull,
    #[error("Oracle confidence interval exceeds the price")]
    OracleConfidenceExceedsRate,
    #[error("Liquidation did not meet market requirements")]
    LiquidationDidNotMeetRequirements,
    #[error("Fee is too high")]
    FeeTooHigh,
    #[error("Share accounting overflow")]
    SharesOverflow,
    #[error("Invalid protocol authority nomination")]
    InvalidNomination,
    #[error("Cannot modify share price when there are zero shares")]
    CantModifySharePriceIfZeroShares,
    #[error("Interest rate cannot be negative")]
    NegativeInterestRate,
    #[error("Cannot socialize debt for a healthy position")]
    CannotSocializeDebtForHealthyPosition,
    #[error("Unsupported mint decimals")]
    UnsupportedMintDecimals,
    #[error("Invalid oracle configuration")]
    InvalidOracleConfig,
    #[error("A capital sweep is already pending for this market")]
    CapitalSweepPending,
    #[error("No capital sweep is pending for this market")]
    NoCapitalSweepPending,
    #[error("Capital sweep position is insolvent")]
    CapitalSweepPositionInsolvent,
    #[error("Capital sweep did not meet market requirements")]
    CapitalSweepDidNotMeetRequirements,
    LiquidatorAlreadyWhitelisted,
    LiquidatorNotWhitelisted,
}

impl LendingError {
    pub fn with_context(
        self,
        location: &'static std::panic::Location<'static>,
    ) -> ErrorWithContext<LendingError> {
        ErrorWithContext::new(self, location)
    }
}

impl<T: std::fmt::Display> From<T> for ErrorWithContext<T> {
    #[track_caller]
    fn from(error: T) -> Self {
        Self::new(error, std::panic::Location::caller())
    }
}

pub trait LendingResultExt: Sized {
    #[track_caller]
    fn track_caller(self) -> Self;

    fn with_msg(self, msg: impl Into<std::borrow::Cow<'static, str>>) -> Self;
}

impl<T> LendingResultExt for LendingResult<T> {
    #[inline(always)]
    fn track_caller(self) -> Self {
        let caller = std::panic::Location::caller();
        self.map_err(|mut err| {
            err.stack.push(DisplayLocation(caller));
            err
        })
    }

    #[inline(always)]
    fn with_msg(self, msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        self.map_err(|mut err| {
            err.msg.push(DisplayCow(msg.into()));
            err
        })
    }
}

#[macro_export]
macro_rules! with_context {
    ( $error:expr) => {{
        let caller = std::panic::Location::caller();
        || $error.with_context(caller)
    }};
}

#[macro_export]
macro_rules! map_context {
    ($error:expr) => {{
        let caller = std::panic::Location::caller();
        |_| $error.with_context(caller)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lending_error_messages_are_user_readable() {
        let cases = [
            (
                LendingError::MaxLtvReached,
                "Borrow would exceed the market max loan-to-value (LTV)",
            ),
            (
                LendingError::PositionIsHealthy,
                "Position is healthy and cannot be liquidated",
            ),
            (
                LendingError::OracleRateTooOld,
                "Oracle price is too old",
            ),
            (
                LendingError::CapitalSweepPending,
                "A capital sweep is already pending for this market",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn error_with_context_displays_inner_message() {
        let err: ErrorWithContext<LendingError> = LendingError::MaxSupplyReached.into();
        let rendered = err.to_string();
        assert!(
            rendered.contains("Supply would exceed the market max supply"),
            "unexpected display: {rendered}"
        );
    }
}
