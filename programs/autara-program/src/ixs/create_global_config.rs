use arch_program::{
    account::{next_account_info, AccountInfo},
    bpf_loader::{LoaderState, LoaderStatus, BPF_LOADER_ID},
    pubkey::Pubkey,
};
use autara_program_lib::accounts::{
    program::{Program, SystemProgram},
    signer::Signer,
};

use crate::error::{LendingAccountValidationError, LendingProgramResult};

pub struct CreateGlobalConfigAccounts<'a, 'b> {
    pub payer: Signer<'a, 'b>,
    pub global_config: &'b AccountInfo<'a>,
    pub system_program: Program<'a, 'b, SystemProgram>,
    /// This program's own on-chain account, owned by the BPF loader. Read to
    /// recover the current upgrade authority so `validate` can gate who is
    /// allowed to create the global config (see the comment on `validate`).
    pub program: &'b AccountInfo<'a>,
}

impl<'a, 'b> CreateGlobalConfigAccounts<'a, 'b> {
    #[inline(never)]
    pub fn from_accounts(
        accounts: &mut impl Iterator<Item = &'b AccountInfo<'a>>,
    ) -> LendingProgramResult<Self>
    where
        'a: 'b,
    {
        let this = Self {
            payer: next_account_info(accounts)?.try_into()?,
            global_config: next_account_info(accounts)?,
            system_program: next_account_info(accounts)?.try_into()?,
            program: next_account_info(accounts)?,
        };
        this.validate()?;
        Ok(this)
    }

    /// `find_global_config_pda` derives the global config address from the
    /// program id alone (no user-supplied seed), so the address is publicly
    /// known before the account ever exists. The processor creates it with a
    /// plain `system_instruction::create_account`, which is first-writer-wins:
    /// whoever's `CreateGlobalConfig` transaction lands first permanently
    /// becomes `GlobalConfig::admin` and `fee_receiver` for every market that
    /// references this config (`GlobalConfig::initialize` accepts both as
    /// caller-supplied instruction data with no further checks). Every other
    /// admin action — `update_global_config`, `redeem_protocol_fees` — is then
    /// gated on `admin`/`fee_receiver` alone, so without this check anyone
    /// could front-run the real deployment and permanently take over protocol
    /// fee collection and admin rights.
    ///
    /// Require the transaction to be signed by the program's current upgrade
    /// authority instead: on Arch the program account itself (owned by the
    /// BPF loader) stores that authority as the first 32 bytes of its data
    /// (`arch_program::bpf_loader::LoaderState`), so it cannot be front-run by
    /// anyone who does not already control the program's upgrade authority
    /// key. `admin`/`fee_receiver` can still be set to any address by that
    /// call, same as today — only who may make the call has changed.
    fn validate(&self) -> LendingProgramResult<()> {
        if self.program.key != &crate::id() || self.program.owner != &BPF_LOADER_ID {
            return Err(LendingAccountValidationError::NotProgramUpgradeAuthority.into());
        }
        let data = self.program.try_borrow_data()?;
        if data.len() < LoaderState::program_data_offset() {
            return Err(LendingAccountValidationError::NotProgramUpgradeAuthority.into());
        }
        // Once `status` is `Finalized`, `LoaderState::authority_address_or_next_version`
        // stops meaning "signer who can send program management instructions" and is
        // repurposed as a forwarding address for the next program version (see the
        // field's own doc comment in `arch_program::bpf_loader`). Reject that case
        // explicitly rather than silently comparing `payer` against a value that is no
        // longer an authority at all.
        let status = u64::from_le_bytes(
            data[32..LoaderState::program_data_offset()]
                .try_into()
                .unwrap(),
        );
        if status == LoaderStatus::Finalized as u64 {
            return Err(LendingAccountValidationError::NotProgramUpgradeAuthority.into());
        }
        let upgrade_authority = Pubkey::from_slice(&data[..32]);
        if self.payer.key != &upgrade_authority {
            return Err(LendingAccountValidationError::NotProgramUpgradeAuthority.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ixs::test_utils::{create_signer, create_system_program, AccountInfoWrapper};
    use arch_program::pubkey::Pubkey;

    /// A program account owned by the BPF loader, with `authority` stamped at
    /// the front of its data the way `arch_program::bpf_loader::LoaderState`
    /// lays it out (authority: Pubkey, then an 8-byte status).
    fn program_account_with_authority(authority: Pubkey) -> AccountInfoWrapper {
        let key = Box::leak(Box::new(crate::id()));
        let lamports = Box::leak(Box::new(1_000_000u64));
        let mut data = vec![0u8; LoaderState::program_data_offset()];
        data[..32].copy_from_slice(&authority.0);
        let account_data = Box::leak(Box::new(data));
        AccountInfoWrapper(AccountInfo::new(
            key,
            lamports,
            account_data,
            Box::leak(Box::new(BPF_LOADER_ID)),
            Box::leak(Box::new(Default::default())),
            false,
            false,
            true,
        ))
    }

    /// Same as `program_account_with_authority`, but with `status` stamped
    /// `Finalized` — at that point the leading 32 bytes are a forwarding
    /// address for the next program version, not a signer authority.
    fn finalized_program_account(authority_or_next_version: Pubkey) -> AccountInfoWrapper {
        let key = Box::leak(Box::new(crate::id()));
        let lamports = Box::leak(Box::new(1_000_000u64));
        let mut data = vec![0u8; LoaderState::program_data_offset()];
        data[..32].copy_from_slice(&authority_or_next_version.0);
        data[32..LoaderState::program_data_offset()]
            .copy_from_slice(&(LoaderStatus::Finalized as u64).to_le_bytes());
        let account_data = Box::leak(Box::new(data));
        AccountInfoWrapper(AccountInfo::new(
            key,
            lamports,
            account_data,
            Box::leak(Box::new(BPF_LOADER_ID)),
            Box::leak(Box::new(Default::default())),
            false,
            false,
            true,
        ))
    }

    fn accounts(
        payer: &AccountInfoWrapper,
        global_config: &AccountInfoWrapper,
        system_program: &AccountInfoWrapper,
        program: &AccountInfoWrapper,
    ) -> Vec<AccountInfo<'static>> {
        vec![
            payer.0.clone(),
            global_config.0.clone(),
            system_program.0.clone(),
            program.0.clone(),
        ]
    }

    #[test]
    fn accepts_signature_from_the_program_upgrade_authority() {
        let payer = create_signer();
        let global_config = create_signer();
        let system_program = create_system_program();
        let program = program_account_with_authority(*payer.key);
        let accounts = accounts(&payer, &global_config, &system_program, &program);

        CreateGlobalConfigAccounts::from_accounts(&mut accounts.iter()).unwrap();
    }

    #[test]
    fn rejects_a_payer_that_is_not_the_upgrade_authority() {
        let payer = create_signer();
        let global_config = create_signer();
        let system_program = create_system_program();
        // Front-running attempt: the program's real upgrade authority is
        // someone else, but the attacker still signs as `payer`.
        let real_authority = Pubkey::new_unique();
        let program = program_account_with_authority(real_authority);
        let accounts = accounts(&payer, &global_config, &system_program, &program);

        let result = CreateGlobalConfigAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected a payer that is not the upgrade authority to be rejected");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::NotProgramUpgradeAuthority
        );
    }

    #[test]
    fn rejects_a_program_account_not_owned_by_the_bpf_loader() {
        let payer = create_signer();
        let global_config = create_signer();
        let system_program = create_system_program();
        let mut program = program_account_with_authority(*payer.key);
        program.mutate_owner();
        let accounts = accounts(&payer, &global_config, &system_program, &program);

        let result = CreateGlobalConfigAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected a non-loader-owned program account to be rejected");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::NotProgramUpgradeAuthority
        );
    }

    #[test]
    fn rejects_a_program_account_at_the_wrong_address() {
        let payer = create_signer();
        let global_config = create_signer();
        let system_program = create_system_program();
        let mut program = program_account_with_authority(*payer.key);
        program.0.key = Box::leak(Box::new(Pubkey::new_unique()));
        let accounts = accounts(&payer, &global_config, &system_program, &program);

        let result = CreateGlobalConfigAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected a program account at the wrong address to be rejected");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::NotProgramUpgradeAuthority
        );
    }

    #[test]
    fn rejects_a_finalized_program_even_if_payer_matches_the_stored_bytes() {
        let payer = create_signer();
        let global_config = create_signer();
        let system_program = create_system_program();
        // The stored bytes equal `payer`'s key, but once `status` is
        // `Finalized` those bytes mean "next version", not "authority" --
        // this must still be rejected.
        let program = finalized_program_account(*payer.key);
        let accounts = accounts(&payer, &global_config, &system_program, &program);

        let result = CreateGlobalConfigAccounts::from_accounts(&mut accounts.iter());
        let Err(error) = result else {
            panic!("expected a finalized program account to be rejected");
        };
        assert_eq!(
            error,
            LendingAccountValidationError::NotProgramUpgradeAuthority
        );
    }
}
