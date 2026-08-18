use arch_program::program_error::ProgramError;
use arch_program::pubkey::Pubkey;
use autara_lib::error::{DisplayCow, ErrorWithContext, LendingError};
use autara_lib::token::pubkey_base58;
use autara_program_lib::accounts::AccountValidationError;
use num_enum::{IntoPrimitive, TryFromPrimitive};

pub type LendingProgramResult<T = ()> = Result<T, LendingProgramError>;

#[derive(Debug, Clone)]
pub struct LendingProgramError(pub ErrorWithContext<LendingProgramErrorKind>);

impl LendingProgramError {
    pub fn with_msg(mut self, msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        self.0.msg.push(DisplayCow(msg.into()));
        self
    }
}

pub fn validation_err(
    err: LendingAccountValidationError,
    msg: impl Into<std::borrow::Cow<'static, str>>,
) -> LendingProgramError {
    LendingProgramError::from(err).with_msg(msg)
}

pub fn format_pubkey_pair(label_a: &str, a: &Pubkey, label_b: &str, b: &Pubkey) -> String {
    format!(
        "{label_a}={} {label_b}={}",
        pubkey_base58(a),
        pubkey_base58(b)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LendingProgramErrorKind {
    ProgramError(ProgramError),
    AccountValidationError(AccountValidationError),
    LendingAccountValidationError(LendingAccountValidationError),
    LendingError(LendingError),
}

impl LendingProgramErrorKind {
    pub fn from_error_code(code: u32) -> Self {
        match ProgramError::from(code as u64) {
            ProgramError::Custom(custom) => if custom >= ACCOUNT_VALIDATION_ERROR_OFFSET
                && custom < LENDING_ACCOUNT_VALIDATION_ERROR_OFFSET
            {
                let err_code = custom - ACCOUNT_VALIDATION_ERROR_OFFSET;
                AccountValidationError::try_from(err_code as u8)
                    .map(LendingProgramErrorKind::AccountValidationError)
                    .ok()
            } else if custom >= LENDING_ACCOUNT_VALIDATION_ERROR_OFFSET
                && custom < LENDING_ERROR_OFFSET
            {
                let err_code = custom - LENDING_ACCOUNT_VALIDATION_ERROR_OFFSET;
                LendingAccountValidationError::try_from(err_code as u8)
                    .map(LendingProgramErrorKind::LendingAccountValidationError)
                    .ok()
            } else if custom >= LENDING_ERROR_OFFSET {
                let err_code = custom - LENDING_ERROR_OFFSET;
                LendingError::try_from(err_code as u8)
                    .map(LendingProgramErrorKind::LendingError)
                    .ok()
            } else {
                Some(Self::ProgramError(ProgramError::Custom(custom)))
            }
            .unwrap_or_else(|| LendingProgramErrorKind::ProgramError(ProgramError::Custom(custom))),
            err => Self::ProgramError(err),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum LendingAccountValidationError {
    InvalidAuthority,
    InvalidMarketAuthority,
    InvalidMarket,
    InvalidMarketVault,
    InvalidMintForTokenAccount,
    InvalidProtocolAuthority,
}

pub const ACCOUNT_VALIDATION_ERROR_OFFSET: u32 = 6000;
pub const LENDING_ACCOUNT_VALIDATION_ERROR_OFFSET: u32 = 6500;
pub const LENDING_ERROR_OFFSET: u32 = 7000;

impl Into<ProgramError> for LendingProgramError {
    fn into(self) -> ProgramError {
        #[cfg(feature = "entrypoint")]
        {
            arch_program::msg!("{:?}", self);
        }
        match self.0.error {
            LendingProgramErrorKind::ProgramError(err) => err,
            LendingProgramErrorKind::AccountValidationError(err) => {
                ProgramError::Custom(ACCOUNT_VALIDATION_ERROR_OFFSET + err as u32)
            }
            LendingProgramErrorKind::LendingError(err) => {
                ProgramError::Custom(LENDING_ERROR_OFFSET + err as u32)
            }
            LendingProgramErrorKind::LendingAccountValidationError(err) => {
                ProgramError::Custom(LENDING_ACCOUNT_VALIDATION_ERROR_OFFSET + err as u32)
            }
        }
    }
}

impl From<AccountValidationError> for LendingProgramErrorKind {
    fn from(err: AccountValidationError) -> Self {
        LendingProgramErrorKind::AccountValidationError(err)
    }
}

impl From<ProgramError> for LendingProgramErrorKind {
    fn from(err: ProgramError) -> Self {
        LendingProgramErrorKind::ProgramError(err)
    }
}

impl From<LendingError> for LendingProgramErrorKind {
    fn from(err: LendingError) -> Self {
        LendingProgramErrorKind::LendingError(err)
    }
}

impl From<LendingAccountValidationError> for LendingProgramErrorKind {
    fn from(err: LendingAccountValidationError) -> Self {
        LendingProgramErrorKind::LendingAccountValidationError(err)
    }
}

impl<T> From<ErrorWithContext<T>> for LendingProgramError
where
    LendingProgramErrorKind: From<T>,
{
    fn from(err: ErrorWithContext<T>) -> Self {
        LendingProgramError(ErrorWithContext {
            error: LendingProgramErrorKind::from(err.error),
            msg: err.msg,
            stack: err.stack,
        })
    }
}

impl<T> From<T> for LendingProgramError
where
    LendingProgramErrorKind: From<T>,
{
    #[track_caller]
    fn from(err: T) -> Self {
        LendingProgramError(ErrorWithContext::new(
            LendingProgramErrorKind::from(err),
            std::panic::Location::caller(),
        ))
    }
}

impl<T> PartialEq<T> for LendingProgramError
where
    LendingProgramErrorKind: PartialEq<T>,
{
    fn eq(&self, other: &T) -> bool {
        self.0.error.eq(other)
    }
}

impl PartialEq<ProgramError> for LendingProgramErrorKind {
    fn eq(&self, other: &ProgramError) -> bool {
        match &self {
            LendingProgramErrorKind::ProgramError(err) => err == other,
            _ => false,
        }
    }
}

impl PartialEq<AccountValidationError> for LendingProgramErrorKind {
    fn eq(&self, other: &AccountValidationError) -> bool {
        match &self {
            LendingProgramErrorKind::AccountValidationError(err) => err == other,
            _ => false,
        }
    }
}
impl PartialEq<LendingAccountValidationError> for LendingProgramErrorKind {
    fn eq(&self, other: &LendingAccountValidationError) -> bool {
        match &self {
            LendingProgramErrorKind::LendingAccountValidationError(err) => err == other,
            _ => false,
        }
    }
}

impl PartialEq<LendingError> for LendingProgramErrorKind {
    fn eq(&self, other: &LendingError) -> bool {
        match &self {
            LendingProgramErrorKind::LendingError(err) => err == other,
            _ => false,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use arch_program::program_error::ProgramError;
    use autara_lib::error::LendingError;
    use autara_program_lib::accounts::AccountValidationError;

    use crate::error::{
        LendingAccountValidationError, LendingProgramError, LendingProgramErrorKind,
    };

    #[test]
    pub fn check_into_from_error_code() {
        let errors: [LendingProgramErrorKind; 3] = [
            AccountValidationError::InvalidOwner.into(),
            LendingAccountValidationError::InvalidMarket.into(),
            LendingError::InvalidMarketForPosition.into(),
        ];
        for error in errors {
            let program_error: ProgramError = LendingProgramError::from(error.clone()).into();
            let ProgramError::Custom(code) = program_error else {
                panic!("Expected ProgramError::Custom");
            };
            let converted_error = LendingProgramErrorKind::from_error_code(code as u32);
            assert_eq!(error, converted_error);
        }
    }
}
