use arch_program::pubkey::Pubkey;

#[inline(always)]
pub fn market_seed_without_bump<'a>(
    curator: &'a Pubkey,
    supply_mint: &'a Pubkey,
    collateral_mint: &'a Pubkey,
    index: &'a [u8; 1],
) -> [&'a [u8]; 5] {
    [
        b"market",
        curator.as_ref(),
        supply_mint.as_ref(),
        collateral_mint.as_ref(),
        index,
    ]
}

#[inline(always)]
pub fn market_seed_with_bump<'a>(
    curator: &'a Pubkey,
    supply_mint: &'a Pubkey,
    collateral_mint: &'a Pubkey,
    index: &'a [u8; 1],
    bump: &'a [u8; 1],
) -> [&'a [u8]; 6] {
    [
        b"market",
        curator.as_ref(),
        supply_mint.as_ref(),
        collateral_mint.as_ref(),
        index,
        bump,
    ]
}

#[inline(always)]
pub fn supply_position_seed<'a>(market: &'a Pubkey, authority: &'a Pubkey) -> [&'a [u8]; 3] {
    [b"supply_position", market.as_ref(), authority.as_ref()]
}

#[inline(always)]
pub fn supply_position_seed_with_bump<'a>(
    market: &'a Pubkey,
    authority: &'a Pubkey,
    bump: &'a [u8; 1],
) -> [&'a [u8]; 4] {
    [
        b"supply_position",
        market.as_ref(),
        authority.as_ref(),
        bump,
    ]
}

#[inline(always)]
pub fn borrow_position_seed<'a>(market: &'a Pubkey, authority: &'a Pubkey) -> [&'a [u8]; 3] {
    [b"borrow_position", market.as_ref(), authority.as_ref()]
}

#[inline(always)]
pub fn borrow_position_seed_with_bump<'a>(
    market: &'a Pubkey,
    authority: &'a Pubkey,
    bump: &'a [u8; 1],
) -> [&'a [u8]; 4] {
    [
        b"borrow_position",
        market.as_ref(),
        authority.as_ref(),
        bump,
    ]
}

#[inline(always)]
pub fn find_supply_position_pda(
    program_id: &Pubkey,
    market: &Pubkey,
    authority: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(&supply_position_seed(market, authority), program_id)
}

#[inline(always)]
pub fn find_borrow_position_pda(
    program_id: &Pubkey,
    market: &Pubkey,
    authority: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(&borrow_position_seed(market, authority), program_id)
}

#[inline(always)]
pub fn liquidator_whitelist_entry_seed<'a>(
    market: &'a Pubkey,
    liquidator: &'a Pubkey,
) -> [&'a [u8]; 3] {
    [
        b"liquidator_whitelist",
        market.as_ref(),
        liquidator.as_ref(),
    ]
}

#[inline(always)]
pub fn liquidator_whitelist_entry_seed_with_bump<'a>(
    market: &'a Pubkey,
    liquidator: &'a Pubkey,
    bump: &'a [u8; 1],
) -> [&'a [u8]; 4] {
    [
        b"liquidator_whitelist",
        market.as_ref(),
        liquidator.as_ref(),
        bump,
    ]
}

#[inline(always)]
pub fn find_liquidator_whitelist_entry_pda(
    program_id: &Pubkey,
    market: &Pubkey,
    liquidator: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &liquidator_whitelist_entry_seed(market, liquidator),
        program_id,
    )
}

#[inline(always)]
pub fn find_market_pda(
    program_id: &Pubkey,
    curator: &Pubkey,
    supply_mint: &Pubkey,
    collateral_mint: &Pubkey,
    index: u8,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &market_seed_without_bump(curator, supply_mint, collateral_mint, &[index]),
        program_id,
    )
}

#[inline(always)]
pub fn global_config_seed() -> [&'static [u8]; 1] {
    [b"global_config"]
}

#[inline(always)]
pub fn global_config_seed_with_bump(bump: &[u8; 1]) -> [&[u8]; 2] {
    [b"global_config", bump]
}

#[inline(always)]
pub fn find_global_config_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&global_config_seed(), program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liquidator_whitelist_pda_is_market_and_liquidator_specific() {
        let program_id = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let liquidator = Pubkey::new_unique();

        let (pda, bump) = find_liquidator_whitelist_entry_pda(&program_id, &market, &liquidator);
        assert_eq!(
            Pubkey::create_program_address(
                &liquidator_whitelist_entry_seed_with_bump(&market, &liquidator, &[bump]),
                &program_id,
            )
            .unwrap(),
            pda,
        );

        assert_ne!(
            pda,
            find_liquidator_whitelist_entry_pda(&program_id, &Pubkey::new_unique(), &liquidator,).0,
        );
        assert_ne!(
            pda,
            find_liquidator_whitelist_entry_pda(&program_id, &market, &Pubkey::new_unique(),).0,
        );
    }
}
