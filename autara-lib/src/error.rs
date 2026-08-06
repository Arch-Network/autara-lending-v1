use std::ops::Deref;

use num_enum::{IntoPrimitive, TryFromPrimitive};

pub type LendingResult<T = ()> = Result<T, ErrorWithContext<LendingError>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("error = {error:?}, msg = {msg:?}, stack = {stack:?}")]
pub struct ErrorWithContext<T> {
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

impl<T> ErrorWithContext<T> {
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

impl<T> Deref for ErrorWithContext<T> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum LendingError {
    MathOverflow,
    AdditionOverflow,
    SubtractionOverflow,
    MultiplicationOverflow,
    DivisionOverflow,
    DivisionByZero,
    CastOverflow,
    MaxLtvReached,
    MaxUtilisationRateReached,
    InvalidMarketForPosition,
    PositionIsHealthy,
    MaxSupplyReached,
    InvalidLtvConfig,
    InvalidCurve,
    InvalidExpArg,
    InvalidMaxUtilisationRate,
    InvalidLiquidationLtvShouldDecrease,
    InvalidPythOracleAccount,
    InvalidChaosOracleAccount,
    InvalidOracleFeedId,
    FailedToLoadAccount,
    WithdrawalExceedsReserves,
    WithdrawalExceedsDeposited,
    RepayExceedsBorrowed,
    OracleRateTooOld,
    OracleRateRelativeConfidenceTooLow,
    NegativeOracleRate,
    OracleRateIsNull,
    OracleConfidenceExceedsRate,
    LiquidationDidNotMeetRequirements,
    FeeTooHigh,
    SharesOverflow,
    InvalidNomination,
    CantModifySharePriceIfZeroShares,
    NegativeInterestRate,
    CannotSocializeDebtForHealthyPosition,
    UnsupportedMintDecimals,
    InvalidOracleConfig,
}

impl LendingError {
    pub fn with_context(
        self,
        location: &'static std::panic::Location<'static>,
    ) -> ErrorWithContext<LendingError> {
        ErrorWithContext::new(self, location)
    }
}

impl<T> From<T> for ErrorWithContext<T> {
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
