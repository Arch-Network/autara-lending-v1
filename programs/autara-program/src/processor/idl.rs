//! On-chain IDL account instruction handler — lets the program publish its own
//! IDL on-chain (the canonical alternative to importing the IDL into the indexer).
//!
//! This is a **native port** of the IDL-account protocol that Arch's Satellite
//! framework (`arch-satellite-lang`) injects into every `#[program]`. Autara is a
//! raw `arch_program` program, so we cannot inherit those handlers from the
//! `#[program]` macro — we reproduce them here against raw `arch_program`
//! primitives. The on-wire protocol (selector, sub-op encoding, account layout,
//! and PDA derivation) is reproduced byte-for-byte so that the stock
//! `arch-cli idl` tool and the arch-rust-indexer IDL decoder both interoperate
//! with it.
//!
//! Sources mirrored (Arch-Network/satellite, branch `main`):
//!   - `lang/src/idl.rs`                         (selector, IdlInstruction, IdlAccount)
//!   - `lang/syn/src/codegen/program/idl.rs`     (handler bodies)
//!   - `lang/syn/src/codegen/program/dispatch.rs`(selector check)
//!   - `lang/attribute/account/src/lib.rs`       (account discriminator scheme)
//!
//! Account layout written here (matches arch-rust-indexer
//! `shared/src/idl/onchain.rs`, `HEADER_LEN = 44`):
//!   [0..8]   8-byte account discriminator
//!   [8..40]  authority: Pubkey (32 bytes)
//!   [40..44] data_len: u32 little-endian
//!   [44..]   zlib-compressed IDL JSON
//!
//! Coverage: the sub-ops have no on-chain integration test — `autara-integration-tests`
//! drives the LIVE testnet deployment, so exercising Create/Write/Close there
//! would mutate the real IDL account. They are covered end-to-end by
//! `publish_idl` against the deployed program instead.

use arch_program::{
    account::AccountInfo, program::invoke_signed, program_error::ProgramError, pubkey::Pubkey,
    rent::minimum_rent, system_instruction, system_program::SYSTEM_PROGRAM_ID,
};
use borsh::BorshDeserialize;

use crate::error::LendingProgramResult;

/// 8-byte selector that marks an instruction as targeting the IDL handler.
///
/// `Sha256("anchor:idl")[..8]` == `IDL_IX_TAG = 0x0a69e9a778bcf440` (little-endian).
/// Source: `arch-satellite-lang/lang/src/idl.rs`. Being 8 bytes, it can never
/// collide with `AurataInstruction`'s 1-byte tags (0..=21).
pub const IDL_IX_TAG_LE: [u8; 8] = [0x40, 0xf4, 0xbc, 0x78, 0xa7, 0xe9, 0x69, 0x0a];

/// 8-byte account discriminator Satellite assigns to `IdlAccount`.
/// `IdlAccount` is declared `#[account("internal")]`, and Satellite derives the
/// discriminator as `Sha256("internal:IdlAccount")[..8]`
/// (`gen_discriminator(namespace, name)` in `lang/syn/.../program/common.rs`).
///
/// Confirmed against `sha256("internal:IdlAccount")[..8] == 184662bf3a907b9e`.
pub const IDL_ACCOUNT_DISCRIMINATOR: [u8; 8] = [0x18, 0x46, 0x62, 0xbf, 0x3a, 0x90, 0x7b, 0x9e];

/// Anchor seed for the canonical IDL account.
const IDL_SEED: &str = "anchor:idl";

const DISCRIMINATOR_LEN: usize = 8;
const AUTHORITY_OFFSET: usize = DISCRIMINATOR_LEN; // 8
const DATA_LEN_OFFSET: usize = DISCRIMINATOR_LEN + 32; // 40
const HEADER_LEN: usize = DISCRIMINATOR_LEN + 32 + 4; // 44
const MAX_SPACE_PER_OP: usize = 10_000;
const ERASED_AUTHORITY: [u8; 32] = [0u8; 32];

/// Borsh-encoded sub-operations (after the 8-byte selector).
/// Mirrors `IdlInstruction` in `arch-satellite-lang/lang/src/idl.rs`.
/// `new_authority` is decoded as raw bytes (borsh-identical to `Pubkey`) to avoid
/// depending on `Pubkey: BorshDeserialize`.
#[derive(BorshDeserialize)]
enum IdlInstruction {
    Create { data_len: u64 },
    CreateBuffer,
    Write { data: Vec<u8> },
    SetBuffer,
    SetAuthority { new_authority: [u8; 32] },
    Close,
    Resize { data_len: u64 },
}

/// True when `instruction_data` is an IDL instruction (starts with the selector).
#[inline]
pub fn is_idl_instruction(instruction_data: &[u8]) -> bool {
    instruction_data.starts_with(&IDL_IX_TAG_LE)
}

/// Entry point. `data` is the instruction payload with the 8-byte selector
/// already stripped by the dispatcher in `lib.rs`.
/// `#[inline(never)]` for the same reason as every `process_*` handler: keep
/// the dispatcher's stack frame within the 4KB SBPF limit.
#[inline(never)]
pub fn process_idl_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> LendingProgramResult {
    let ix =
        IdlInstruction::try_from_slice(data).map_err(|_| ProgramError::InvalidInstructionData)?;
    let mut iter = accounts.iter();
    match ix {
        IdlInstruction::Create { data_len } => idl_create_account(program_id, &mut iter, data_len),
        IdlInstruction::CreateBuffer => idl_create_buffer(program_id, &mut iter),
        IdlInstruction::Write { data } => idl_write(program_id, &mut iter, data),
        IdlInstruction::SetBuffer => idl_set_buffer(program_id, &mut iter),
        IdlInstruction::SetAuthority { new_authority } => {
            idl_set_authority(program_id, &mut iter, new_authority)
        }
        IdlInstruction::Resize { data_len } => idl_resize_account(program_id, &mut iter, data_len),
        IdlInstruction::Close => idl_close_account(program_id, &mut iter),
    }
}

// --- helpers ---------------------------------------------------------------

type AccIter<'a, 'info> = std::slice::Iter<'a, AccountInfo<'info>>;

fn next<'a, 'info>(iter: &mut AccIter<'a, 'info>) -> Result<&'a AccountInfo<'info>, ProgramError> {
    iter.next().ok_or(ProgramError::NotEnoughAccountKeys)
}

/// `(idl_address, base, bump)` for this program.
fn derive_idl(program_id: &Pubkey) -> Result<(Pubkey, Pubkey, u8), ProgramError> {
    let (base, bump) = Pubkey::find_program_address(&[], program_id);
    let idl = Pubkey::create_with_seed(&base, IDL_SEED, program_id)
        .map_err(|_| ProgramError::InvalidArgument)?;
    Ok((idl, base, bump))
}

/// Validate the canonical IDL account: owned by us, at the derived address, with
/// our discriminator.
fn check_canonical_idl(program_id: &Pubkey, idl: &AccountInfo) -> Result<(), ProgramError> {
    if idl.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let (expected, _, _) = derive_idl(program_id)?;
    if idl.key != &expected {
        return Err(ProgramError::InvalidArgument);
    }
    let data = idl.try_borrow_data()?;
    if data.len() < HEADER_LEN || data[..DISCRIMINATOR_LEN] != IDL_ACCOUNT_DISCRIMINATOR {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

/// A program-owned IdlAccount (used for write-buffers, which are NOT at the
/// canonical address). Validates owner + discriminator only.
fn check_owned_idl(program_id: &Pubkey, acc: &AccountInfo) -> Result<(), ProgramError> {
    if acc.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let data = acc.try_borrow_data()?;
    if data.len() < HEADER_LEN || data[..DISCRIMINATOR_LEN] != IDL_ACCOUNT_DISCRIMINATOR {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

/// `has_one = authority` + signer + not-erased, against an already validated
/// IdlAccount.
fn check_authority(idl: &AccountInfo, authority: &AccountInfo) -> Result<(), ProgramError> {
    if !authority.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if authority.key.0 == ERASED_AUTHORITY {
        return Err(ProgramError::InvalidArgument);
    }
    let data = idl.try_borrow_data()?;
    if data[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32] != authority.key.0 {
        return Err(ProgramError::InvalidArgument);
    }
    Ok(())
}

fn read_data_len(data: &[u8]) -> usize {
    u32::from_le_bytes(data[DATA_LEN_OFFSET..HEADER_LEN].try_into().unwrap()) as usize
}

fn write_data_len(data: &mut [u8], len: usize) {
    data[DATA_LEN_OFFSET..HEADER_LEN].copy_from_slice(&(len as u32).to_le_bytes());
}

// --- sub-ops ---------------------------------------------------------------

/// One-time creation of the canonical IDL account at
/// `create_with_seed(find_program_address([], program_id), "anchor:idl", program_id)`,
/// signed by the program's base PDA. Accounts: from, to, base, system_program, program.
fn idl_create_account(
    program_id: &Pubkey,
    iter: &mut AccIter<'_, '_>,
    data_len: u64,
) -> LendingProgramResult {
    let from = next(iter)?;
    let to = next(iter)?;
    let base = next(iter)?;
    let system_program = next(iter)?;
    let program = next(iter)?;

    if program.key != program_id {
        return Err(ProgramError::IncorrectProgramId.into());
    }
    if !from.is_signer {
        return Err(ProgramError::MissingRequiredSignature.into());
    }
    let (expected_idl, expected_base, bump) = derive_idl(program_id)?;
    if to.key != &expected_idl || base.key != &expected_base {
        return Err(ProgramError::InvalidArgument.into());
    }

    let space = core::cmp::min(HEADER_LEN + data_len as usize, MAX_SPACE_PER_OP);
    let lamports = minimum_rent(space);
    // Signer seeds for `base` = empty seeds + bump (mirrors Satellite `&[&[nonce][..]]`).
    let bump_seed: &[u8] = &[bump];
    let signer_seeds: &[&[u8]] = &[bump_seed];

    let ix = system_instruction::create_account_with_seed(
        from.key,
        to.key,
        base.key,
        IDL_SEED,
        lamports,
        space as u64,
        program_id,
    );
    // No anchoring UTXO is needed here: `create_account_with_seed` under the
    // base PDA's signature is how arch-cli drives this, and it succeeded against
    // the live testnet deployment.
    invoke_signed(
        &ix,
        &[
            from.clone(),
            to.clone(),
            base.clone(),
            system_program.clone(),
        ],
        &[signer_seeds],
    )?;

    // Initialize header: discriminator, authority = payer, data_len = 0.
    let mut data = to.try_borrow_mut_data()?;
    if data.len() < HEADER_LEN {
        return Err(ProgramError::AccountDataTooSmall.into());
    }
    data[..DISCRIMINATOR_LEN].copy_from_slice(&IDL_ACCOUNT_DISCRIMINATOR);
    data[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32].copy_from_slice(&from.key.0);
    write_data_len(&mut data, 0);
    Ok(())
}

/// Initialize a caller-provided, program-owned, pre-zeroed buffer account.
/// Accounts: buffer, authority.
fn idl_create_buffer(program_id: &Pubkey, iter: &mut AccIter<'_, '_>) -> LendingProgramResult {
    let buffer = next(iter)?;
    let authority = next(iter)?;

    if buffer.owner != program_id {
        return Err(ProgramError::IllegalOwner.into());
    }
    if !authority.is_signer || authority.key.0 == ERASED_AUTHORITY {
        return Err(ProgramError::MissingRequiredSignature.into());
    }
    let mut data = buffer.try_borrow_mut_data()?;
    if data.len() < HEADER_LEN {
        return Err(ProgramError::AccountDataTooSmall.into());
    }
    // `#[account(zero)]`. Satellite tests only the 8-byte discriminator here
    // because every Satellite account carries one; Autara's do not — its state
    // accounts are discriminated by SIZE. A Market whose first eight bytes
    // happen to be zero (bump, index, padding and both fee fields all 0) would
    // pass a discriminator-only gate, and this handler would then stamp an IDL
    // header over its curator/config bytes and let `Write` fill the rest with
    // arbitrary data. Require the WHOLE account to be zeroed instead: true of a
    // freshly allocated buffer, false of anything holding real state.
    if data.iter().any(|b| *b != 0) {
        return Err(ProgramError::AccountAlreadyInitialized.into());
    }
    data[..DISCRIMINATOR_LEN].copy_from_slice(&IDL_ACCOUNT_DISCRIMINATOR);
    data[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32].copy_from_slice(&authority.key.0);
    write_data_len(&mut data, 0);
    Ok(())
}

/// Append `idl_data` to the account's trailing buffer. Accounts: idl, authority.
/// Works for both the canonical account and a write-buffer (owner-checked).
fn idl_write(
    program_id: &Pubkey,
    iter: &mut AccIter<'_, '_>,
    idl_data: Vec<u8>,
) -> LendingProgramResult {
    let idl = next(iter)?;
    let authority = next(iter)?;

    check_owned_idl(program_id, idl)?;
    check_authority(idl, authority)?;

    let mut data = idl.try_borrow_mut_data()?;
    let prev_len = read_data_len(&data);
    let new_len = prev_len
        .checked_add(idl_data.len())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    if HEADER_LEN + new_len > data.len() {
        // Account must have been Resized large enough first.
        return Err(ProgramError::AccountDataTooSmall.into());
    }
    data[HEADER_LEN + prev_len..HEADER_LEN + new_len].copy_from_slice(&idl_data);
    write_data_len(&mut data, new_len);
    Ok(())
}

/// Copy a write-buffer's contents into the canonical IDL account.
/// Accounts: buffer, idl, authority.
fn idl_set_buffer(program_id: &Pubkey, iter: &mut AccIter<'_, '_>) -> LendingProgramResult {
    let buffer = next(iter)?;
    let idl = next(iter)?;
    let authority = next(iter)?;

    check_owned_idl(program_id, buffer)?;
    check_canonical_idl(program_id, idl)?;
    check_authority(idl, authority)?;
    // buffer.authority must match idl.authority.
    {
        let b = buffer.try_borrow_data()?;
        let i = idl.try_borrow_data()?;
        if b[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32] != i[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32]
        {
            return Err(ProgramError::InvalidArgument.into());
        }
    }

    let buffer_data = buffer.try_borrow_data()?;
    let buffer_len = read_data_len(&buffer_data);
    let mut idl_data = idl.try_borrow_mut_data()?;
    if HEADER_LEN + buffer_len > idl_data.len() {
        return Err(ProgramError::AccountDataTooSmall.into());
    }
    idl_data[HEADER_LEN..HEADER_LEN + buffer_len]
        .copy_from_slice(&buffer_data[HEADER_LEN..HEADER_LEN + buffer_len]);
    write_data_len(&mut idl_data, buffer_len);
    Ok(())
}

/// Set a new authority on the canonical IDL account. Accounts: idl, authority.
fn idl_set_authority(
    program_id: &Pubkey,
    iter: &mut AccIter<'_, '_>,
    new_authority: [u8; 32],
) -> LendingProgramResult {
    let idl = next(iter)?;
    let authority = next(iter)?;

    check_owned_idl(program_id, idl)?;
    check_authority(idl, authority)?;

    let mut data = idl.try_borrow_mut_data()?;
    data[AUTHORITY_OFFSET..AUTHORITY_OFFSET + 32].copy_from_slice(&new_authority);
    Ok(())
}

/// Grow the canonical IDL account (rent top-up + realloc), in <=10kb steps, only
/// while it still has no IDL data written. Accounts: idl, authority, system_program.
fn idl_resize_account(
    program_id: &Pubkey,
    iter: &mut AccIter<'_, '_>,
    data_len: u64,
) -> LendingProgramResult {
    let idl = next(iter)?;
    let authority = next(iter)?;
    let system_program = next(iter)?;

    check_canonical_idl(program_id, idl)?;
    check_authority(idl, authority)?;

    // Refuse to grow accounts that already contain IDL bytes (mirrors Satellite).
    {
        let data = idl.try_borrow_data()?;
        if read_data_len(&data) != 0 {
            return Err(ProgramError::AccountAlreadyInitialized.into());
        }
    }

    let current_space = idl.data_len();
    let target = data_len as usize;
    let growth = core::cmp::min(target.saturating_sub(current_space), MAX_SPACE_PER_OP);
    let new_space = current_space
        .checked_add(growth)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    if new_space > current_space {
        let new_rent = minimum_rent(new_space);
        let topup = new_rent.saturating_sub(idl.lamports());
        if topup > 0 {
            let ix = system_instruction::transfer(authority.key, idl.key, topup);
            invoke_signed(
                &ix,
                &[authority.clone(), idl.clone(), system_program.clone()],
                &[],
            )?;
        }
        idl.realloc(new_space, false)?;
    }
    Ok(())
}

/// Close the canonical IDL account, returning lamports to `destination`.
/// Accounts: account, authority, destination.
fn idl_close_account(program_id: &Pubkey, iter: &mut AccIter<'_, '_>) -> LendingProgramResult {
    let account = next(iter)?;
    let authority = next(iter)?;
    let destination = next(iter)?;

    check_owned_idl(program_id, account)?;
    check_authority(account, authority)?;

    // Move all lamports to the destination.
    {
        let mut from_lamports = account.try_borrow_mut_lamports()?;
        let mut to_lamports = destination.try_borrow_mut_lamports()?;
        **to_lamports = to_lamports
            .checked_add(**from_lamports)
            .ok_or(ProgramError::ArithmeticOverflow)?;
        **from_lamports = 0;
    }
    // Hand the account back to the system program and drop its data, mirroring
    // `arch-satellite-lang`'s `common::close`. Wiping only the discriminator
    // would leave a zero-prefixed, program-owned account sitting at the
    // canonical address, which `CreateBuffer` would then happily re-initialize
    // under a new authority — letting anyone re-publish the program's IDL.
    {
        let mut data = account.try_borrow_mut_data()?;
        for b in data.iter_mut() {
            *b = 0;
        }
    }
    account.assign(&SYSTEM_PROGRAM_ID);
    account.realloc(0, false)?;
    Ok(())
}
