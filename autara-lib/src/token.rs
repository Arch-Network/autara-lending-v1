use arch_program::{
    account::AccountMeta, instruction::Instruction, pubkey::Pubkey,
    system_program::SYSTEM_PROGRAM_ID,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    pub mint: Pubkey,
    pub decimals: u8,
}

impl TokenInfo {
    pub fn get_associated_token_address(&self, owner: &Pubkey) -> Pubkey {
        get_associated_token_address(owner, &self.mint)
    }
}

/// Create an associated token account, succeeding as a no-op when a valid ATA
/// already exists (Arch ATA program instruction data `[1]` / #2491).
///
/// Local wire-compatible builder until `create_associated_token_account_idempotent`
/// is published on crates.io.
pub fn create_ata_ix(
    funder_info: &Pubkey,
    associated_token_account_info: Option<&Pubkey>,
    owner_account_info: &Pubkey,
    spl_token_mint_info: &Pubkey,
) -> Instruction {
    let associated_token_account_info = if let Some(info) = associated_token_account_info {
        *info
    } else {
        get_associated_token_address(owner_account_info, spl_token_mint_info)
    };

    Instruction::new(
        apl_associated_token_account::id(),
        vec![1],
        vec![
            AccountMeta::new(*funder_info, true),
            AccountMeta::new(associated_token_account_info, false),
            AccountMeta::new_readonly(*owner_account_info, false),
            AccountMeta::new_readonly(*spl_token_mint_info, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(apl_token::id(), false),
        ],
    )
}

pub fn get_associated_token_address(
    wallet_address: &Pubkey,
    spl_token_mint_address: &Pubkey,
) -> Pubkey {
    apl_associated_token_account::get_associated_token_address_and_bump_seed(
        wallet_address,
        spl_token_mint_address,
        &apl_associated_token_account::id(),
    )
    .0
}

/// Human-readable base58 form for logs/API (Arch Pubkey Display is still hex).
pub fn pubkey_base58(pubkey: &Pubkey) -> String {
    bs58::encode(pubkey.serialize()).into_string()
}
