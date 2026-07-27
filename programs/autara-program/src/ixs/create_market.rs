use apl_token::state::Mint;
use arch_program::account::{next_account_info, AccountInfo};
use autara_lib::state::global_config::GlobalConfig;
use autara_lib::token::get_associated_token_address;
use autara_program_lib::accounts::{
    packed::PackedOwnedAccount,
    program::{Program, SystemProgram},
    signer::Signer,
    token::{AplAssociatedTokenProgram, AplTokenProgram},
    zero_copy::ZeroCopyOwnedAccount,
};

use crate::{
    error::{LendingAccountValidationError, LendingProgramResult},
    state::AutaraAccount,
};

pub struct CreateMarketAccounts<'a, 'b> {
    pub curator: Signer<'a, 'b>,
    pub payer: Signer<'a, 'b>,
    pub global_config: ZeroCopyOwnedAccount<'a, 'b, AutaraAccount<GlobalConfig>>,
    pub market: &'b AccountInfo<'a>,
    pub supply_mint: PackedOwnedAccount<'a, 'b, Mint>,
    pub supply_vault: &'b AccountInfo<'a>,
    pub collateral_mint: PackedOwnedAccount<'a, 'b, Mint>,
    pub collateral_vault: &'b AccountInfo<'a>,
    pub apl_token_program: Program<'a, 'b, AplTokenProgram>,
    pub associated_token_program: Program<'a, 'b, AplAssociatedTokenProgram>,
    pub system_program: Program<'a, 'b, SystemProgram>,
}

impl<'a, 'b> CreateMarketAccounts<'a, 'b> {
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            curator: next_account_info(accounts)?.try_into()?,
            payer: next_account_info(accounts)?.try_into()?,
            global_config: next_account_info(accounts)?.try_into()?,
            market: next_account_info(accounts)?,
            supply_mint: next_account_info(accounts)?.try_into()?,
            supply_vault: next_account_info(accounts)?,
            collateral_mint: next_account_info(accounts)?.try_into()?,
            collateral_vault: next_account_info(accounts)?,
            apl_token_program: next_account_info(accounts)?.try_into()?,
            associated_token_program: next_account_info(accounts)?.try_into()?,
            system_program: next_account_info(accounts)?.try_into()?,
        };
        this.validate()?;
        Ok(this)
    }

    pub fn validate(&self) -> LendingProgramResult<()> {
        let (expected_global_config, _) = autara_lib::pda::find_global_config_pda(&crate::id());
        if *self.global_config.key() != expected_global_config {
            return Err(
                crate::error::LendingAccountValidationError::InvalidProtocolAuthority.into(),
            );
        }
        // The vaults must be the market's own associated token accounts. Without this,
        // an already-initialized token account is accepted as-is (the ATA creation in
        // the processor is skipped when the account exists), which would let a curator
        // point a market at a token account they control and take every deposit.
        if *self.supply_vault.key
            != get_associated_token_address(self.market.key, self.supply_mint.key())
        {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        if *self.collateral_vault.key
            != get_associated_token_address(self.market.key, self.collateral_mint.key())
        {
            return Err(LendingAccountValidationError::InvalidMarketVault.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ixs::test_utils::{
        create_associated_token_account, create_mint, create_program_account, create_signer,
        create_token_account_at, create_token_program, AccountInfoWrapper, AutaraAccounts,
    };
    use arch_program::pubkey::Pubkey;

    struct CreateMarketSet {
        curator: AccountInfoWrapper,
        payer: AccountInfoWrapper,
        global_config: AccountInfoWrapper,
        market: AccountInfoWrapper,
        supply_mint: AccountInfoWrapper,
        supply_vault: AccountInfoWrapper,
        collateral_mint: AccountInfoWrapper,
        collateral_vault: AccountInfoWrapper,
        apl_token_program: AccountInfoWrapper,
        associated_token_program: AccountInfoWrapper,
        system_program: AccountInfoWrapper,
    }

    impl CreateMarketSet {
        fn new() -> Self {
            let base = AutaraAccounts::new();
            let market = create_signer();
            let supply_mint_key = Pubkey::new_unique();
            let collateral_mint_key = Pubkey::new_unique();
            Self {
                curator: create_signer(),
                payer: create_signer(),
                global_config: base.global_config,
                supply_vault: create_associated_token_account(market.key, &supply_mint_key),
                collateral_vault: create_associated_token_account(market.key, &collateral_mint_key),
                supply_mint: create_mint(supply_mint_key, 6),
                collateral_mint: create_mint(collateral_mint_key, 8),
                market,
                apl_token_program: create_token_program(),
                associated_token_program: create_program_account(apl_associated_token_account::id()),
                system_program: create_program_account(
                    arch_program::system_program::SYSTEM_PROGRAM_ID,
                ),
            }
        }

        fn validate(&self) -> LendingProgramResult<()> {
            let accounts = [
                self.curator.0.clone(),
                self.payer.0.clone(),
                self.global_config.0.clone(),
                self.market.0.clone(),
                self.supply_mint.0.clone(),
                self.supply_vault.0.clone(),
                self.collateral_mint.0.clone(),
                self.collateral_vault.0.clone(),
                self.apl_token_program.0.clone(),
                self.associated_token_program.0.clone(),
                self.system_program.0.clone(),
            ];
            CreateMarketAccounts::from_accounts(&mut accounts.iter()).map(|_| ())
        }
    }

    #[test]
    fn validate_accepts_canonical_vaults() {
        CreateMarketSet::new().validate().unwrap();
    }

    /// The processor only creates the ATA when the account does not already exist, so an
    /// attacker-owned token account passed here would be adopted verbatim as the market
    /// vault: deposits would land in it and only the attacker could move them out.
    #[test]
    fn validate_rejects_attacker_owned_supply_vault() {
        let mut set = CreateMarketSet::new();
        let attacker = Pubkey::new_unique();
        set.supply_vault =
            create_token_account_at(Pubkey::new_unique(), &attacker, set.supply_mint.key);
        assert_eq!(
            set.validate().unwrap_err(),
            LendingAccountValidationError::InvalidMarketVault
        );
    }

    #[test]
    fn validate_rejects_attacker_owned_collateral_vault() {
        let mut set = CreateMarketSet::new();
        let attacker = Pubkey::new_unique();
        set.collateral_vault =
            create_token_account_at(Pubkey::new_unique(), &attacker, set.collateral_mint.key);
        assert_eq!(
            set.validate().unwrap_err(),
            LendingAccountValidationError::InvalidMarketVault
        );
    }

    /// Another market's (correctly owned) ATA is still not this market's ATA.
    #[test]
    fn validate_rejects_vault_of_another_owner() {
        let mut set = CreateMarketSet::new();
        let other_market = Pubkey::new_unique();
        set.supply_vault = create_associated_token_account(&other_market, set.supply_mint.key);
        assert_eq!(
            set.validate().unwrap_err(),
            LendingAccountValidationError::InvalidMarketVault
        );
    }

    /// A vault that is the market's ATA but for the wrong mint must also be rejected.
    #[test]
    fn validate_rejects_vault_for_wrong_mint() {
        let mut set = CreateMarketSet::new();
        set.supply_vault = create_associated_token_account(set.market.key, set.collateral_mint.key);
        assert_eq!(
            set.validate().unwrap_err(),
            LendingAccountValidationError::InvalidMarketVault
        );
    }

    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Only the market's own associated token account may serve as a vault. The
            /// dangerous shape is an already-initialized token account at an arbitrary
            /// address whose token-owner is the attacker: the processor skips ATA creation
            /// for existing accounts, so without this check it would be adopted verbatim.
            #[test]
            fn any_non_canonical_supply_vault_is_rejected(
                vault_key in prop::array::uniform32(0u8..=255u8),
                token_owner in prop::array::uniform32(0u8..=255u8),
            ) {
                let mut set = CreateMarketSet::new();
                let key = Pubkey::from(vault_key);
                prop_assume!(
                    key != get_associated_token_address(set.market.key, set.supply_mint.key)
                );
                set.supply_vault =
                    create_token_account_at(key, &Pubkey::from(token_owner), set.supply_mint.key);
                prop_assert_eq!(
                    set.validate().unwrap_err(),
                    LendingAccountValidationError::InvalidMarketVault
                );
            }

            #[test]
            fn any_non_canonical_collateral_vault_is_rejected(
                vault_key in prop::array::uniform32(0u8..=255u8),
                token_owner in prop::array::uniform32(0u8..=255u8),
            ) {
                let mut set = CreateMarketSet::new();
                let key = Pubkey::from(vault_key);
                prop_assume!(
                    key != get_associated_token_address(set.market.key, set.collateral_mint.key)
                );
                set.collateral_vault = create_token_account_at(
                    key,
                    &Pubkey::from(token_owner),
                    set.collateral_mint.key,
                );
                prop_assert_eq!(
                    set.validate().unwrap_err(),
                    LendingAccountValidationError::InvalidMarketVault
                );
            }

            /// An ATA is only valid for the exact (owner, mint) pair it was derived from,
            /// so another wallet's ATA — even a perfectly well-formed one — is rejected.
            #[test]
            fn ata_of_any_other_owner_is_rejected(
                other_owner in prop::array::uniform32(0u8..=255u8),
            ) {
                let mut set = CreateMarketSet::new();
                let other = Pubkey::from(other_owner);
                prop_assume!(other != *set.market.key);
                set.supply_vault = create_associated_token_account(&other, set.supply_mint.key);
                prop_assert_eq!(
                    set.validate().unwrap_err(),
                    LendingAccountValidationError::InvalidMarketVault
                );
            }

            /// The market's own ATA for some unrelated mint is still the wrong account.
            #[test]
            fn market_ata_for_any_other_mint_is_rejected(
                other_mint in prop::array::uniform32(0u8..=255u8),
            ) {
                let mut set = CreateMarketSet::new();
                let mint = Pubkey::from(other_mint);
                prop_assume!(mint != *set.supply_mint.key);
                set.supply_vault = create_associated_token_account(set.market.key, &mint);
                prop_assert_eq!(
                    set.validate().unwrap_err(),
                    LendingAccountValidationError::InvalidMarketVault
                );
            }

            /// Positive control: the canonical pair is always accepted, so the check cannot
            /// be satisfied by simply rejecting everything.
            #[test]
            fn canonical_vaults_are_always_accepted(_seed in 0u8..=255u8) {
                prop_assert!(CreateMarketSet::new().validate().is_ok());
            }
        }
    }
}
