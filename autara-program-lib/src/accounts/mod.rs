use arch_program::pubkey::Pubkey;
use num_enum::{IntoPrimitive, TryFromPrimitive};

pub mod borsh;
pub mod packed;
pub mod program;
pub mod signer;
pub mod token;
pub mod zero_copy;

pub trait OwnedAccount {
    fn is_valid_owner(owner: &Pubkey) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoPrimitive, TryFromPrimitive, thiserror::Error)]
#[repr(u8)]
pub enum AccountValidationError {
    #[error("Account is not a signer")]
    NotSigner,
    #[error("Account data is invalid")]
    InvalidData,
    #[error("Account has an invalid owner")]
    InvalidOwner,
    #[error("Account key is invalid")]
    InvalidKey,
    #[error("Account is already loaded")]
    AlreadyLoaded,
    #[error("Account is not writable")]
    NotWritable,
    #[error("Account is not initialized")]
    AccountNotInitialized,
}
