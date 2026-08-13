use anyhow::{Context, Result, bail};
use apl_token::state::Account as TokenAccount;
use arch_sdk::{
    AsyncArchRpcClient,
    arch_program::{program_pack::Pack, pubkey::Pubkey},
};

pub fn positive_balance_delta(before: u64, after: u64) -> Option<u64> {
    after.checked_sub(before).filter(|delta| *delta > 0)
}

pub fn rate_within_slippage(
    initial_in: u64,
    initial_out: u64,
    fresh_in: u64,
    fresh_out: u64,
    slippage_bps: u16,
) -> bool {
    if initial_in == 0
        || initial_out == 0
        || fresh_in == 0
        || fresh_out == 0
        || slippage_bps > 10_000
    {
        return false;
    }
    let Some(left) = u128::from(fresh_out)
        .checked_mul(u128::from(initial_in))
        .and_then(|value| value.checked_mul(10_000))
    else {
        return false;
    };
    let Some(right) = u128::from(initial_out)
        .checked_mul(u128::from(fresh_in))
        .and_then(|value| value.checked_mul(u128::from(10_000 - slippage_bps)))
    else {
        return false;
    };
    left >= right
}

pub async fn read_token_balance(
    rpc: &AsyncArchRpcClient,
    owner: Pubkey,
    mint: Pubkey,
) -> Result<u64> {
    let ata = autara_lib::token::get_associated_token_address(&owner, &mint);
    let accounts = rpc
        .get_multiple_accounts(vec![ata])
        .await
        .context("failed to read token balance")?;
    let Some(account) = accounts.into_iter().next().flatten() else {
        return Ok(0);
    };
    if account.key != ata || account.owner != apl_token::id() {
        bail!("associated token account identity or program owner mismatch");
    }
    let token =
        TokenAccount::unpack(&account.data).context("failed to decode associated token account")?;
    if token.owner != owner || token.mint != mint {
        bail!("associated token account contents do not match owner and mint");
    }
    Ok(token.amount)
}

#[cfg(test)]
mod tests {
    use super::{positive_balance_delta, rate_within_slippage};

    #[test]
    fn isolates_only_newly_seized_inventory() {
        assert_eq!(positive_balance_delta(500, 725), Some(225));
        assert_eq!(positive_balance_delta(500, 500), None);
        assert_eq!(positive_balance_delta(500, 499), None);
    }

    #[test]
    fn compares_rates_for_different_exact_inputs() {
        assert!(rate_within_slippage(100, 1_000, 50, 495, 100));
        assert!(!rate_within_slippage(100, 1_000, 50, 494, 100));
        assert!(rate_within_slippage(3, 10, 6, 20, 0));
        assert!(!rate_within_slippage(0, 10, 6, 20, 100));
        assert!(!rate_within_slippage(3, 0, 6, 20, 100));
        assert!(!rate_within_slippage(3, 10, 0, 20, 100));
        assert!(!rate_within_slippage(3, 10, 6, 0, 100));
        assert!(!rate_within_slippage(3, 10, 6, 20, 10_001));
    }

    #[test]
    fn overflow_fails_closed() {
        assert!(!rate_within_slippage(
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            1,
        ));
    }
}
