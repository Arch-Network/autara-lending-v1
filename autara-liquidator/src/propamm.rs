//! PropAMM liquidity path (RFQ vault AMM) for the liquidator.
//!
//! PropAMM's on-chain `ExecuteTrade` requires the `quote_signer` to co-sign the
//! transaction, so — unlike the CLAMM whirlpool swap — it CANNOT be embedded as a
//! CPI callback inside the lending `liquidate` instruction (a callback only inherits
//! the outer tx's signers, and the backend never signs the liquidator's liquidate tx).
//! Therefore PropAMM runs as a SEPARATE swap transaction after a no-callback liquidation.
//!
//! We hold the quote_signer key locally (testnet operator), so we build + sign the
//! swap ourselves as `[quote_signer, user]` — mirroring prop-amm/src/bin/swap_test.rs —
//! instead of the fragile path of splicing a user signature into the backend's partial tx.
//!
//! Pricing: fetched from the PropAMM backend `GET /health` (`current_price`). The on-chain
//! program additionally applies a ±10% inventory-skew adjustment to the payout; we quote the
//! nominal `base*price` for routing, which is correct here because PropAMM's price is far above
//! the CLAMM pool's effective price (the skew band can't flip the decision).

use anyhow::{Context, Result};
use arch_sdk::arch_program::{
    account::AccountMeta,
    bitcoin::{key::Keypair, Network},
    instruction::Instruction,
    pubkey::Pubkey,
    sanitized::ArchMessage,
    system_program::SYSTEM_PROGRAM_ID,
};
use arch_sdk::{build_and_sign_transaction, ArchRpcClient, Status};
use autara_lib::token::get_associated_token_address;
use borsh::{BorshDeserialize, BorshSerialize};

// ---- on-chain types (replicated from propammprogram to avoid an arch_program version pin clash) ----

#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct Quote {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub side: Side,
    pub base_amount: u64,
    pub quote_amount: u64,
    pub user_pubkey: Pubkey,
    pub expiry_ts: u128,
    pub nonce: u64,
}

#[derive(BorshSerialize)]
enum PropAmmInstruction {
    #[allow(dead_code)]
    InitializeConfig { max_quote_ttl_ms: u64 },
    #[allow(dead_code)]
    CreateVault,
    ExecuteTrade { quote: Quote },
}

#[allow(clippy::too_many_arguments)]
fn execute_trade_instruction(
    quote: Quote,
    program: Pubkey,
    config: Pubkey,
    quote_signer: Pubkey,
    user: Pubkey,
    user_nonce: Pubkey,
    base_vault: Pubkey,
    quote_vault: Pubkey,
    base_user_ata: Pubkey,
    quote_user_ata: Pubkey,
    base_mint: Pubkey,
    quote_mint: Pubkey,
) -> Instruction {
    Instruction {
        program_id: program,
        accounts: vec![
            AccountMeta::new(config, false),
            AccountMeta::new(quote_signer, true),
            AccountMeta::new(user, true),
            AccountMeta::new(user_nonce, false),
            AccountMeta::new(base_vault, false),
            AccountMeta::new(quote_vault, false),
            AccountMeta::new(base_user_ata, false),
            AccountMeta::new(quote_user_ata, false),
            AccountMeta::new(base_mint, false),
            AccountMeta::new(quote_mint, false),
            AccountMeta::new(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(apl_token::id(), false),
        ],
        data: borsh::to_vec(&PropAmmInstruction::ExecuteTrade { quote }).unwrap(),
    }
}

// ---- liquidator-side PropAMM client ----

#[derive(Clone)]
pub struct PropAmm {
    pub program_id: Pubkey,
    pub config_pubkey: Pubkey,
    pub quote_signer_kp: Keypair,
    pub quote_signer_pk: Pubkey,
    pub base_mint: Pubkey,  // tBTC
    pub quote_mint: Pubkey, // tUSDC
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub base_decimals: u32,
    pub quote_decimals: u32,
    pub backend_url: String,
}

impl PropAmm {
    fn user_nonce_pda(&self, user: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"user_nonce", self.config_pubkey.as_ref(), user.as_ref()],
            &self.program_id,
        )
        .0
    }

    /// True if this venue can swap `collateral_mint -> supply_mint`.
    pub fn supports(&self, collateral_mint: &Pubkey, supply_mint: &Pubkey) -> bool {
        (*collateral_mint == self.base_mint && *supply_mint == self.quote_mint)
            || (*collateral_mint == self.quote_mint && *supply_mint == self.base_mint)
    }

    /// Live price (quote per base, USD) from the backend health endpoint.
    pub async fn fetch_price(&self) -> Result<f64> {
        let url = format!("{}/health", self.backend_url.trim_end_matches('/'));
        let v: serde_json::Value = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await?
            .json()
            .await?;
        v.get("current_price")
            .and_then(|p| p.as_f64())
            .context("backend /health missing current_price")
    }

    /// price scaled to micro-USD (price * 1e6) as integer, for exact amount math.
    fn price_micro(price: f64) -> u128 {
        (price * 1_000_000.0).round() as u128
    }

    /// Nominal output (in supply-token atoms) of swapping `amount_in` of `collateral_mint`
    /// into `supply_mint` at `price`. Returns (Side, base_amount, quote_amount, out_atoms).
    /// `out_atoms` is the supply-token amount used for routing comparison.
    pub fn quote(
        &self,
        collateral_mint: &Pubkey,
        supply_mint: &Pubkey,
        amount_in: u64,
        price: f64,
    ) -> Option<(Side, u64, u64, u64)> {
        let pm = Self::price_micro(price);
        if pm == 0 || amount_in == 0 {
            return None;
        }
        let base_dec = self.base_decimals;
        let quote_dec = self.quote_decimals;
        if *collateral_mint == self.base_mint && *supply_mint == self.quote_mint {
            // SELL base -> quote: input is base_amount, output quote.
            // quote_atoms = base_atoms * price_micro * 10^quote_dec / (1e6 * 10^base_dec)
            let num = (amount_in as u128) * pm * 10u128.pow(quote_dec);
            let den = 1_000_000u128 * 10u128.pow(base_dec);
            let quote_amount = (num / den) as u64;
            if quote_amount == 0 {
                return None;
            }
            Some((Side::Sell, amount_in, quote_amount, quote_amount))
        } else if *collateral_mint == self.quote_mint && *supply_mint == self.base_mint {
            // BUY quote -> base: input is quote_amount, output base.
            // base_atoms = quote_atoms * 1e6 * 10^base_dec / (price_micro * 10^quote_dec)
            let num = (amount_in as u128) * 1_000_000u128 * 10u128.pow(base_dec);
            let den = pm * 10u128.pow(quote_dec);
            let base_amount = (num / den) as u64;
            if base_amount == 0 {
                return None;
            }
            Some((Side::Buy, base_amount, amount_in, base_amount))
        } else {
            None
        }
    }

    /// Execute a `collateral_mint -> supply_mint` swap of `amount_in` as a standalone tx,
    /// signed by [quote_signer, user]. Returns the supply-token output (nominal) on success.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_swap(
        &self,
        arch_client: &ArchRpcClient,
        user_kp: &Keypair,
        user_pk: Pubkey,
        collateral_mint: &Pubkey,
        supply_mint: &Pubkey,
        amount_in: u64,
        price: f64,
        network: Network,
    ) -> Result<u64> {
        let (side, base_amount, quote_amount, out) = self
            .quote(collateral_mint, supply_mint, amount_in, price)
            .context("propamm cannot quote this pair/amount")?;

        let now_ms: u128 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        let quote = Quote {
            base_mint: self.base_mint,
            quote_mint: self.quote_mint,
            side,
            base_amount,
            quote_amount,
            user_pubkey: user_pk,
            expiry_ts: now_ms + 20_000,
            nonce: now_ms as u64,
        };

        let base_user_ata = get_associated_token_address(&user_pk, &self.base_mint);
        let quote_user_ata = get_associated_token_address(&user_pk, &self.quote_mint);
        let ix = execute_trade_instruction(
            quote,
            self.program_id,
            self.config_pubkey,
            self.quote_signer_pk,
            user_pk,
            self.user_nonce_pda(&user_pk),
            self.base_vault,
            self.quote_vault,
            base_user_ata,
            quote_user_ata,
            self.base_mint,
            self.quote_mint,
        );

        let blockhash = arch_client.get_best_block_hash().await?.try_into()?;
        let message = ArchMessage::new(&[ix], Some(user_pk), blockhash);
        let tx = build_and_sign_transaction(
            message,
            vec![self.quote_signer_kp, *user_kp],
            network,
        )?;
        let txids = arch_client.send_transactions(vec![tx]).await?;
        let processed = arch_client.wait_for_processed_transactions(txids).await?;
        let result = processed.first().context("propamm: no tx processed")?;
        if result.status != Status::Processed {
            anyhow::bail!(
                "propamm swap failed: status={:?} logs={:?}",
                result.status,
                result.logs
            );
        }
        Ok(out)
    }
}
