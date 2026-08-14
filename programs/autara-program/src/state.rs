use arch_program::pubkey::Pubkey;
use autara_lib::state::{
    borrow_position::BorrowPosition, global_config::GlobalConfig,
    liquidator_whitelist::LiquidatorWhitelistEntry, market::Market,
    supply_position::SupplyPosition,
};
use autara_program_lib::accounts::{
    program::ProgramAccount, zero_copy::ZeroCopyInitialized, OwnedAccount,
};
use bytemuck::{Pod, Zeroable};
use std::ops::{Deref, DerefMut};

#[repr(transparent)]
#[derive(Pod, Zeroable, Clone, Copy, Debug)]
pub struct AutaraAccount<T>(T);

impl<T> OwnedAccount for AutaraAccount<T> {
    fn is_valid_owner(owner: &arch_program::pubkey::Pubkey) -> bool {
        *owner == crate::id()
    }
}

const ZEROED_PUBKEY: Pubkey = Pubkey([0; 32]);

impl ZeroCopyInitialized for AutaraAccount<Market> {
    fn is_initialized(&self) -> bool {
        self.0.config().curator() != &ZEROED_PUBKEY
    }
}

impl ZeroCopyInitialized for AutaraAccount<BorrowPosition> {
    fn is_initialized(&self) -> bool {
        self.0.authority() != &ZEROED_PUBKEY
    }
}

impl ZeroCopyInitialized for AutaraAccount<SupplyPosition> {
    fn is_initialized(&self) -> bool {
        self.0.authority() != &ZEROED_PUBKEY
    }
}

impl ZeroCopyInitialized for AutaraAccount<GlobalConfig> {
    fn is_initialized(&self) -> bool {
        self.0.admin() != &ZEROED_PUBKEY
    }
}

impl ZeroCopyInitialized for AutaraAccount<LiquidatorWhitelistEntry> {
    fn is_initialized(&self) -> bool {
        self.0.market() != &ZEROED_PUBKEY
    }
}

impl<T> Deref for AutaraAccount<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for AutaraAccount<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[repr(transparent)]
#[derive(Pod, Zeroable, Clone, Copy, Debug)]
pub struct AutaraUninitializedAccount<T>(AutaraAccount<T>);

impl<T> Deref for AutaraUninitializedAccount<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0 .0
    }
}

impl<T> DerefMut for AutaraUninitializedAccount<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0 .0
    }
}

impl<T> ZeroCopyInitialized for AutaraUninitializedAccount<T>
where
    AutaraAccount<T>: ZeroCopyInitialized,
{
    fn is_initialized(&self) -> bool {
        !self.0.is_initialized()
    }
}

impl<T> OwnedAccount for AutaraUninitializedAccount<T> {
    fn is_valid_owner(owner: &arch_program::pubkey::Pubkey) -> bool {
        *owner == crate::id()
    }
}

pub struct AutaraProgram;

impl ProgramAccount for AutaraProgram {
    fn is_valid_key(key: &Pubkey) -> bool {
        *key == crate::id()
    }
}
