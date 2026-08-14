use arch_program::{
    account::AccountInfo, clock::Clock, program::invoke_signed_unchecked, pubkey::Pubkey,
    rent::minimum_rent, system_instruction,
};
use autara_lib::{
    ixs::CreateMarketInstruction, pda::market_seed_with_bump, state::market::Market,
    token::create_ata_ix,
};
use autara_program_lib::accounts::zero_copy::ZeroCopyOwnedAccountMut;

use crate::{
    error::LendingProgramResult, ixs::CreateMarketAccounts, state::AutaraUninitializedAccount,
};

#[inline(never)]
pub fn process_create_market(
    create_market_accounts: &CreateMarketAccounts,
    data: &CreateMarketInstruction,
    accounts: &[AccountInfo],
    program_id: &Pubkey,
    clock: &Clock,
) -> LendingProgramResult {
    let bump = [data.market_bump];
    let index = [data.index];
    let seed = market_seed_with_bump(
        create_market_accounts.curator.key,
        create_market_accounts.supply_mint.key(),
        create_market_accounts.collateral_mint.key(),
        &index,
        &bump,
    );
    invoke_signed_unchecked(
        &system_instruction::create_account(
            create_market_accounts.payer.key,
            create_market_accounts.market.key,
            minimum_rent(std::mem::size_of::<Market>()),
            std::mem::size_of::<Market>() as u64,
            program_id,
        ),
        accounts,
        &[&seed],
    )?;
    if create_market_accounts.supply_vault.owner != create_market_accounts.apl_token_program.key {
        invoke_signed_unchecked(
            &create_ata_ix(
                create_market_accounts.payer.key,
                Some(create_market_accounts.supply_vault.key),
                create_market_accounts.market.key,
                create_market_accounts.supply_mint.key(),
            ),
            accounts,
            &[],
        )?;
    }
    if create_market_accounts.collateral_vault.owner != create_market_accounts.apl_token_program.key
    {
        invoke_signed_unchecked(
            &create_ata_ix(
                create_market_accounts.payer.key,
                Some(create_market_accounts.collateral_vault.key),
                create_market_accounts.market.key,
                create_market_accounts.collateral_mint.key(),
            ),
            accounts,
            &[],
        )?;
    }
    let market = ZeroCopyOwnedAccountMut::<AutaraUninitializedAccount<Market>>::try_from(
        create_market_accounts.market,
    )?;
    let mut market_ref = market.load_mut();
    market_ref.config_mut().initialize(
        data.market_bump,
        data.index,
        create_market_accounts.curator.key,
        &data.ltv_config,
        data.max_utilisation_rate,
        u64::MAX,
        data.lending_market_fee_in_bps,
        &create_market_accounts.global_config.load_ref(),
    )?;
    market_ref.initlize_supply_vault(
        *create_market_accounts.supply_mint.key(),
        create_market_accounts.supply_mint.decimals as u64,
        *create_market_accounts.supply_vault.key,
        data.supply_oracle_config,
        data.interest_rate,
        clock.unix_timestamp,
    )?;
    market_ref.initialize_collateral_vault(
        *create_market_accounts.collateral_mint.key(),
        create_market_accounts.collateral_mint.decimals as u64,
        *create_market_accounts.collateral_vault.key,
        data.collateral_oracle_config,
    )?;
    Ok(())
}
