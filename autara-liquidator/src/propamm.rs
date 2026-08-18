use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use arch_sdk::{
    RuntimeTransaction, Signature,
    arch_program::{
        bitcoin::{Network, key::Keypair},
        hash::Hash,
        pubkey::Pubkey,
        sanitize::Sanitize,
        system_program::SYSTEM_PROGRAM_ID,
    },
    sign_message_bip322,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::{
    config::{PropAmmConfig, parse_hex_pubkey},
    venue::{Venue, VenueQuote},
};

const BASIS_POINTS: u128 = 10_000;
const PROPAMM_FP_SCALE: u128 = 1_000_000;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum RfqSide {
    Buy,
    Sell,
}

impl RfqSide {
    fn request_value(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EstimatedQuote {
    pub base_amount: u64,
    pub quote_amount: u64,
    pub adjustment_fp: u128,
    pub vault_observed_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct RfqQuote {
    transaction: RuntimeTransaction,
    pub side: RfqSide,
    pub amount_in: u64,
    pub estimated_out: u64,
    pub expiry_ts: u128,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedSide {
    side: RfqSide,
    amount_in: u64,
    estimated_out: u64,
}

#[derive(Debug, Clone, Copy)]
struct ServiceMetadata {
    program_id: Pubkey,
    config_pubkey: Pubkey,
    quote_signer: Pubkey,
    max_quote_ttl_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct MarketMetadata {
    base_mint: Pubkey,
    quote_mint: Pubkey,
    base_vault: Pubkey,
    quote_vault: Pubkey,
}

struct ValidationRequest {
    service: ServiceMetadata,
    market: MarketMetadata,
    user: Pubkey,
    side: RfqSide,
    amount_in: u64,
    estimated_out: u64,
    estimated_quote: EstimatedQuote,
    slippage_bps: u16,
    minimum_expiry_headroom_ms: u64,
    now_ms: u128,
}

#[derive(Debug, Clone)]
pub struct PropAmmClient {
    http: reqwest::Client,
    base_url: String,
    expected_program_id: Pubkey,
    slippage_bps: u16,
    minimum_expiry_headroom_ms: u64,
}

impl PropAmmClient {
    pub fn new(config: &PropAmmConfig) -> Result<Self> {
        if config.slippage_bps > 10_000 {
            bail!("PropAMM slippage_bps must not exceed 10000");
        }
        let base_url = config.base_url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            bail!("PropAMM base_url must not be empty");
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .build()
            .context("failed to build PropAMM HTTP client")?;
        Ok(Self {
            http,
            base_url,
            expected_program_id: parse_hex_pubkey(&config.expected_program_id)
                .context("invalid PropAMM expected_program_id")?,
            slippage_bps: config.slippage_bps,
            minimum_expiry_headroom_ms: config.minimum_expiry_headroom_ms,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn expected_program_id(&self) -> Pubkey {
        self.expected_program_id
    }

    pub fn slippage_bps(&self) -> u16 {
        self.slippage_bps
    }

    pub async fn check_readiness(&self) -> Result<()> {
        let (service, markets) = self.discover().await?;
        if service.program_id != self.expected_program_id {
            bail!("PropAMM readiness returned an unexpected program ID");
        }
        if markets.is_empty() {
            bail!("PropAMM readiness returned no markets");
        }
        Ok(())
    }

    pub async fn quote_exact_in(
        &self,
        input_mint: Pubkey,
        output_mint: Pubkey,
        amount_in: u64,
        user: Pubkey,
    ) -> Result<Option<VenueQuote<RfqQuote>>> {
        if amount_in == 0 || input_mint == output_mint {
            return Ok(None);
        }
        let (service, markets) = self.discover().await?;
        let Some(market) = markets.into_iter().find(|market| {
            (market.base_mint == input_mint && market.quote_mint == output_mint)
                || (market.quote_mint == input_mint && market.base_mint == output_mint)
        }) else {
            return Ok(None);
        };
        let side = if input_mint == market.base_mint {
            RfqSide::Sell
        } else {
            RfqSide::Buy
        };
        let request = QuoteRequest {
            base_mint: hex::encode(market.base_mint.serialize()),
            quote_mint: hex::encode(market.quote_mint.serialize()),
            side: side.request_value(),
            amount: amount_in,
            user_pubkey: hex::encode(user.serialize()),
            slippage_bps: self.slippage_bps,
        };
        let response = self
            .http
            .post(format!("{}/rfq/quote", self.base_url))
            .json(&request)
            .send()
            .await
            .context("PropAMM RFQ quote request failed")?;
        let response = checked_response(response, "PropAMM RFQ quote").await?;
        let response: QuoteResponse = response
            .json()
            .await
            .context("PropAMM RFQ quote response was invalid")?;
        let resolved = resolve_side(
            &market,
            input_mint,
            output_mint,
            amount_in,
            &response.estimated_quote,
        )
        .context("PropAMM RFQ quote did not match the requested pair")?;
        let now_ms = unix_time_ms()?;
        let quote = validate_quote_transaction(
            &response.transaction,
            &ValidationRequest {
                service,
                market,
                user,
                side: resolved.side,
                amount_in: resolved.amount_in,
                estimated_out: resolved.estimated_out,
                estimated_quote: response.estimated_quote,
                slippage_bps: self.slippage_bps,
                minimum_expiry_headroom_ms: self.minimum_expiry_headroom_ms,
                now_ms,
            },
        )?;
        Ok(Some(VenueQuote {
            venue: Venue::PropAmm,
            amount_in,
            estimated_out: quote.estimated_out,
            execution: quote,
        }))
    }

    pub async fn execute_quote(
        &self,
        mut quote: RfqQuote,
        liquidator: &Keypair,
        network: Network,
    ) -> Result<Hash> {
        if !quote.transaction.signatures.is_empty() {
            bail!("PropAMM RFQ quote is already signed");
        }
        if !has_expiry_headroom(
            quote.expiry_ts,
            unix_time_ms()?,
            self.minimum_expiry_headroom_ms,
        ) {
            bail!("PropAMM RFQ quote is too close to expiry");
        }
        quote.transaction.signatures = vec![Signature(sign_message_bip322(
            liquidator,
            &quote.transaction.message.hash(),
            network,
        ))];
        let body = serde_json::to_vec(&quote.transaction)
            .context("failed to serialize signed PropAMM RFQ quote")?;
        let url = format!("{}/rfq/swap", self.base_url);

        loop {
            if !has_expiry_headroom(
                quote.expiry_ts,
                unix_time_ms()?,
                self.minimum_expiry_headroom_ms,
            ) {
                bail!("PropAMM RFQ quote expired while submitting");
            }
            let result = self
                .http
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .await;
            match result {
                Ok(response) if response.status().is_success() => {
                    let response: SwapResponse = response
                        .json()
                        .await
                        .context("PropAMM RFQ swap response was invalid")?;
                    return Hash::from_str(&response.transaction_hash)
                        .context("PropAMM RFQ swap returned an invalid transaction hash");
                }
                Ok(response) => {
                    let status = response.status();
                    let api_error = response.json::<ApiError>().await.ok();
                    let retryable = match api_error.as_ref() {
                        Some(error) => matches!(
                            error.code.as_str(),
                            "rfq_swap_in_progress"
                                | "rfq_submission_failed"
                                | "price_feed_unavailable"
                                | "vault_cache_unavailable"
                        ),
                        None => status.is_server_error(),
                    };
                    if !retryable {
                        let detail = api_error
                            .map(|error| format!("{}: {}", error.code, error.error))
                            .unwrap_or_else(|| status.to_string());
                        bail!("PropAMM RFQ swap rejected: {detail}");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "PropAMM RFQ swap transport error; retrying identical body");
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn discover(&self) -> Result<(ServiceMetadata, Vec<MarketMetadata>)> {
        let health_request = self.http.get(format!("{}/health", self.base_url)).send();
        let markets_request = self.http.get(format!("{}/markets", self.base_url)).send();
        let (health_response, markets_response) = tokio::try_join!(health_request, markets_request)
            .context("PropAMM metadata request failed")?;
        let health: HealthResponse = checked_response(health_response, "PropAMM health")
            .await?
            .json()
            .await
            .context("PropAMM health response was invalid")?;
        if health.status != "ok" || !health.price_feed_active || !health.vault_cache_ready {
            bail!("PropAMM service is not ready");
        }
        let program_id = parse_hex_pubkey(&health.program_pubkey)
            .context("PropAMM health returned an invalid program ID")?;
        if program_id != self.expected_program_id {
            bail!("PropAMM service program ID does not match configuration");
        }
        let quote_signer = parse_hex_pubkey(&health.quote_signer_pubkey)
            .context("PropAMM health returned an invalid quote signer")?;
        let onchain_quote_signer = parse_hex_pubkey(&health.onchain_config.quote_signer)
            .context("PropAMM health returned an invalid on-chain quote signer")?;
        if quote_signer != onchain_quote_signer {
            bail!("PropAMM service and on-chain quote signers differ");
        }
        let config_pubkey = parse_hex_pubkey(&health.onchain_config.config_pubkey)
            .context("PropAMM health returned an invalid config pubkey")?;
        let expected_config = derive_pda(&program_id, &[b"config", quote_signer.as_ref()])?;
        if config_pubkey != expected_config {
            bail!("PropAMM config PDA does not match program and quote signer");
        }

        let markets: MarketsResponse = checked_response(markets_response, "PropAMM markets")
            .await?
            .json()
            .await
            .context("PropAMM markets response was invalid")?;
        let mut decoded_markets = Vec::with_capacity(markets.markets.len());
        for market in markets.markets {
            let base_mint = parse_hex_pubkey(&market.base_mint)
                .context("PropAMM market returned an invalid base mint")?;
            let quote_mint = parse_hex_pubkey(&market.quote_mint)
                .context("PropAMM market returned an invalid quote mint")?;
            let base_vault = parse_hex_pubkey(&market.base_vault)
                .context("PropAMM market returned an invalid base vault")?;
            let quote_vault = parse_hex_pubkey(&market.quote_vault)
                .context("PropAMM market returned an invalid quote vault")?;
            if base_mint == quote_mint {
                bail!("PropAMM market contains identical mints");
            }
            if base_vault
                != derive_pda(
                    &program_id,
                    &[b"vault", base_mint.as_ref(), quote_signer.as_ref()],
                )?
                || quote_vault
                    != derive_pda(
                        &program_id,
                        &[b"vault", quote_mint.as_ref(), quote_signer.as_ref()],
                    )?
            {
                bail!("PropAMM market vault PDA mismatch");
            }
            decoded_markets.push(MarketMetadata {
                base_mint,
                quote_mint,
                base_vault,
                quote_vault,
            });
        }
        Ok((
            ServiceMetadata {
                program_id,
                config_pubkey,
                quote_signer,
                max_quote_ttl_ms: health.onchain_config.max_quote_ttl_ms,
            },
            decoded_markets,
        ))
    }
}

fn resolve_side(
    market: &MarketMetadata,
    input_mint: Pubkey,
    output_mint: Pubkey,
    amount_in: u64,
    estimate: &EstimatedQuote,
) -> Option<ResolvedSide> {
    if amount_in == 0 {
        return None;
    }
    if input_mint == market.base_mint && output_mint == market.quote_mint {
        (estimate.quote_amount > 0).then_some(ResolvedSide {
            side: RfqSide::Sell,
            amount_in,
            estimated_out: estimate.quote_amount,
        })
    } else if input_mint == market.quote_mint && output_mint == market.base_mint {
        (estimate.base_amount > 0).then_some(ResolvedSide {
            side: RfqSide::Buy,
            amount_in,
            estimated_out: estimate.base_amount,
        })
    } else {
        None
    }
}

fn validate_quote_transaction(
    transaction: &RuntimeTransaction,
    expected: &ValidationRequest,
) -> Result<RfqQuote> {
    if transaction.version != 0 {
        bail!("PropAMM RFQ transaction version must be 0");
    }
    if !transaction.signatures.is_empty() {
        bail!("PropAMM RFQ transaction must be unsigned");
    }
    transaction
        .message
        .sanitize()
        .map_err(|error| anyhow!("PropAMM RFQ message is invalid: {error:?}"))?;
    let message = &transaction.message;
    let has_ata_prelude = match (
        message.account_keys.len(),
        message.instructions.len(),
        message.header.num_readonly_unsigned_accounts,
    ) {
        (13, 1, 1) => false,
        (14, 3, 2) => true,
        _ => bail!("PropAMM RFQ transaction has an unexpected shape"),
    };
    if message.header.num_required_signatures != 2
        || message.header.num_readonly_signed_accounts != 0
    {
        bail!("PropAMM RFQ transaction has an unexpected header");
    }
    if message.account_keys.first() != Some(&expected.user)
        || message.account_keys.get(1) != Some(&expected.service.quote_signer)
    {
        bail!("PropAMM RFQ transaction signer set or fee payer is invalid");
    }
    let instruction = if has_ata_prelude {
        validate_idempotent_ata_instruction(
            message,
            &message.instructions[0],
            expected.user,
            expected.market.base_mint,
        )?;
        validate_idempotent_ata_instruction(
            message,
            &message.instructions[1],
            expected.user,
            expected.market.quote_mint,
        )?;
        &message.instructions[2]
    } else {
        &message.instructions[0]
    };
    let program_index = usize::from(instruction.program_id_index);
    if message.account_keys.get(program_index) != Some(&expected.service.program_id)
        || message.is_signer(program_index)
        || message.is_writable_index(program_index)
    {
        bail!("PropAMM RFQ transaction program is invalid");
    }

    let quote = match borsh::from_slice::<PropAmmInstructionWire>(&instruction.data)
        .context("PropAMM RFQ trade instruction could not be decoded")?
    {
        PropAmmInstructionWire::ExecuteTrade { quote, min_out } => (quote, min_out),
        _ => bail!("PropAMM RFQ transaction contains a non-trade instruction"),
    };
    let (wire, min_out) = quote;
    if wire.base_mint != expected.market.base_mint
        || wire.quote_mint != expected.market.quote_mint
        || wire.side != expected.side
        || wire.user_pubkey != expected.user
        || match expected.side {
            RfqSide::Buy => wire.quote_amount != expected.amount_in,
            RfqSide::Sell => wire.base_amount != expected.amount_in,
        }
    {
        bail!("PropAMM RFQ trade fields do not match the request");
    }
    let estimated_base = match expected.side {
        RfqSide::Buy => u128::from(wire.base_amount)
            .checked_mul(PROPAMM_FP_SCALE)
            .context("PropAMM RFQ base estimate overflow")?
            .checked_div(expected.estimated_quote.adjustment_fp)
            .context("PropAMM RFQ adjustment must be positive")?,
        RfqSide::Sell => u128::from(wire.base_amount),
    };
    let estimated_quote = match expected.side {
        RfqSide::Buy => u128::from(wire.quote_amount),
        RfqSide::Sell => {
            u128::from(wire.quote_amount)
                .checked_mul(expected.estimated_quote.adjustment_fp)
                .context("PropAMM RFQ quote estimate overflow")?
                / PROPAMM_FP_SCALE
        }
    };
    if estimated_base != u128::from(expected.estimated_quote.base_amount)
        || estimated_quote != u128::from(expected.estimated_quote.quote_amount)
    {
        bail!("PropAMM RFQ execution estimate does not match the trade adjustment");
    }
    let reported_out = match expected.side {
        RfqSide::Buy => expected.estimated_quote.base_amount,
        RfqSide::Sell => expected.estimated_quote.quote_amount,
    };
    if reported_out != expected.estimated_out {
        bail!("PropAMM RFQ estimated output does not match trade data");
    }
    let minimum_allowed = u128::from(expected.estimated_out)
        .checked_mul(BASIS_POINTS - u128::from(expected.slippage_bps))
        .context("PropAMM RFQ minimum-output calculation overflow")?
        / BASIS_POINTS;
    if u128::from(min_out) != minimum_allowed {
        bail!("PropAMM RFQ minimum output does not match the requested slippage");
    }
    if !has_expiry_headroom(
        wire.expiry_ts,
        expected.now_ms,
        expected.minimum_expiry_headroom_ms,
    ) {
        bail!("PropAMM RFQ quote has insufficient expiry headroom");
    }
    let maximum_expiry = expected
        .now_ms
        .checked_add(u128::from(expected.service.max_quote_ttl_ms))
        .context("PropAMM RFQ maximum-expiry calculation overflow")?;
    if wire.expiry_ts > maximum_expiry {
        bail!("PropAMM RFQ quote exceeds the on-chain maximum TTL");
    }

    let user_nonce = derive_pda(
        &expected.service.program_id,
        &[
            b"user_nonce",
            expected.service.config_pubkey.as_ref(),
            expected.user.as_ref(),
        ],
    )?;
    let expected_accounts = [
        expected.service.config_pubkey,
        expected.service.quote_signer,
        expected.user,
        user_nonce,
        expected.market.base_vault,
        expected.market.quote_vault,
        autara_lib::token::get_associated_token_address(&expected.user, &expected.market.base_mint),
        autara_lib::token::get_associated_token_address(
            &expected.user,
            &expected.market.quote_mint,
        ),
        expected.market.base_mint,
        expected.market.quote_mint,
        SYSTEM_PROGRAM_ID,
        apl_token::id(),
    ];
    if instruction.accounts.len() != expected_accounts.len() {
        bail!("PropAMM RFQ trade account count is invalid");
    }
    for (position, (index, expected_key)) in instruction
        .accounts
        .iter()
        .zip(expected_accounts.iter())
        .enumerate()
    {
        let index = usize::from(*index);
        if message.account_keys.get(index) != Some(expected_key)
            || !message.is_writable_index(index)
        {
            bail!("PropAMM RFQ trade account {position} is invalid");
        }
        let should_sign =
            *expected_key == expected.user || *expected_key == expected.service.quote_signer;
        if message.is_signer(index) != should_sign {
            bail!("PropAMM RFQ trade signer permissions are invalid");
        }
    }

    Ok(RfqQuote {
        transaction: transaction.clone(),
        side: expected.side,
        amount_in: expected.amount_in,
        estimated_out: expected.estimated_out,
        expiry_ts: wire.expiry_ts,
    })
}

fn validate_idempotent_ata_instruction(
    message: &arch_sdk::arch_program::sanitized::ArchMessage,
    instruction: &arch_sdk::arch_program::sanitized::SanitizedInstruction,
    user: Pubkey,
    mint: Pubkey,
) -> Result<()> {
    let expected = autara_lib::token::create_ata_ix(&user, None, &user, &mint);
    let program_index = usize::from(instruction.program_id_index);
    if message.account_keys.get(program_index) != Some(&expected.program_id)
        || message.is_signer(program_index)
        || message.is_writable_index(program_index)
        || instruction.data != expected.data
        || instruction.accounts.len() != expected.accounts.len()
    {
        bail!("PropAMM RFQ ATA prelude is invalid");
    }
    for (index, expected_account) in instruction.accounts.iter().zip(expected.accounts.iter()) {
        let index = usize::from(*index);
        if message.account_keys.get(index) != Some(&expected_account.pubkey) {
            bail!("PropAMM RFQ ATA prelude account is invalid");
        }
    }
    Ok(())
}

#[derive(BorshSerialize, BorshDeserialize)]
enum PropAmmInstructionWire {
    InitializeConfig { max_quote_ttl_ms: u64 },
    CreateVault,
    ExecuteTrade { quote: QuoteWire, min_out: u64 },
}

#[derive(BorshSerialize, BorshDeserialize)]
struct QuoteWire {
    base_mint: Pubkey,
    quote_mint: Pubkey,
    side: RfqSide,
    base_amount: u64,
    quote_amount: u64,
    user_pubkey: Pubkey,
    expiry_ts: u128,
    nonce: u64,
}

#[derive(Serialize)]
struct QuoteRequest<'a> {
    base_mint: String,
    quote_mint: String,
    side: &'a str,
    amount: u64,
    user_pubkey: String,
    slippage_bps: u16,
}

#[derive(Deserialize)]
struct QuoteResponse {
    #[serde(flatten)]
    transaction: RuntimeTransaction,
    estimated_quote: EstimatedQuote,
}

#[derive(Deserialize)]
struct HealthResponse {
    status: String,
    price_feed_active: bool,
    vault_cache_ready: bool,
    program_pubkey: String,
    quote_signer_pubkey: String,
    onchain_config: HealthOnchainConfig,
}

#[derive(Deserialize)]
struct HealthOnchainConfig {
    config_pubkey: String,
    quote_signer: String,
    max_quote_ttl_ms: u64,
}

#[derive(Deserialize)]
struct MarketsResponse {
    markets: Vec<MarketResponse>,
}

#[derive(Deserialize)]
struct MarketResponse {
    base_mint: String,
    quote_mint: String,
    base_vault: String,
    quote_vault: String,
}

#[derive(Deserialize)]
struct SwapResponse {
    transaction_hash: String,
}

#[derive(Deserialize)]
struct ApiError {
    error: String,
    code: String,
}

async fn checked_response(
    response: reqwest::Response,
    operation: &str,
) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let detail = response
        .json::<ApiError>()
        .await
        .map(|error| format!("{}: {}", error.code, error.error))
        .unwrap_or_else(|_| status.to_string());
    bail!("{operation} failed: {detail}")
}

fn derive_pda(program_id: &Pubkey, seeds: &[&[u8]]) -> Result<Pubkey> {
    Pubkey::try_find_program_address(seeds, program_id)
        .map(|(key, _)| key)
        .context("failed to derive PropAMM PDA")
}

fn has_expiry_headroom(expiry_ts: u128, now_ms: u128, minimum_headroom_ms: u64) -> bool {
    now_ms
        .checked_add(u128::from(minimum_headroom_ms))
        .is_some_and(|minimum_expiry| expiry_ts >= minimum_expiry)
}

fn unix_time_ms() -> Result<u128> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use arch_sdk::{
        RuntimeTransaction,
        arch_program::{
            account::AccountMeta, bitcoin::Network, hash::Hash, instruction::Instruction,
            pubkey::Pubkey, sanitized::ArchMessage, system_program::SYSTEM_PROGRAM_ID,
        },
        generate_new_keypair,
    };
    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::post,
    };
    use serde_json::json;

    use super::{
        EstimatedQuote, MarketMetadata, PropAmmClient, RfqQuote, RfqSide, ServiceMetadata,
        ValidationRequest, resolve_side, unix_time_ms, validate_quote_transaction,
    };
    use crate::config::PropAmmConfig;

    #[test]
    fn resolves_sell_and_uses_quote_payout() {
        let market = market();
        let estimated = EstimatedQuote {
            base_amount: 100,
            quote_amount: 12_345,
            adjustment_fp: 1_000_000,
            vault_observed_at_ms: 1,
        };

        let resolved = resolve_side(
            &market,
            market.base_mint,
            market.quote_mint,
            100,
            &estimated,
        )
        .unwrap();

        assert_eq!(resolved.side, RfqSide::Sell);
        assert_eq!(resolved.amount_in, 100);
        assert_eq!(resolved.estimated_out, 12_345);
    }

    #[test]
    fn resolves_buy_and_uses_base_payout() {
        let market = market();
        let estimated = EstimatedQuote {
            base_amount: 99,
            quote_amount: 12_345,
            adjustment_fp: 1_000_000,
            vault_observed_at_ms: 1,
        };

        let resolved = resolve_side(
            &market,
            market.quote_mint,
            market.base_mint,
            12_345,
            &estimated,
        )
        .unwrap();

        assert_eq!(resolved.side, RfqSide::Buy);
        assert_eq!(resolved.amount_in, 12_345);
        assert_eq!(resolved.estimated_out, 99);
    }

    #[test]
    fn rejects_unsupported_pair_and_zero_input() {
        let market = market();
        let estimate = estimate();
        assert!(resolve_side(&market, key(99), market.base_mint, 1, &estimate).is_none());
        assert!(resolve_side(&market, market.base_mint, market.quote_mint, 0, &estimate).is_none());
    }

    #[test]
    fn accepts_exact_unsigned_rfq_transaction() {
        let fixture = fixture(RfqSide::Sell, 100, 1_000, 1_020_000, 990);

        let quote = validate_quote_transaction(&fixture.transaction, &fixture.request).unwrap();

        assert_eq!(quote.side, RfqSide::Sell);
        assert_eq!(quote.amount_in, 100);
        assert_eq!(quote.estimated_out, 1_000);
        assert_eq!(quote.expiry_ts, 1_020_000);
    }

    #[test]
    fn accepts_idempotent_ata_prelude_from_live_rfq_service() {
        let fixture = fixture_with_ata_prelude(RfqSide::Sell, 100, 1_000, 1_020_000, 990);

        let quote = validate_quote_transaction(&fixture.transaction, &fixture.request).unwrap();

        assert_eq!(quote.side, RfqSide::Sell);
        assert_eq!(quote.amount_in, 100);
        assert_eq!(quote.estimated_out, 1_000);
    }

    #[test]
    fn rejects_changed_idempotent_ata_prelude() {
        let fixture = fixture_with_ata_prelude(RfqSide::Sell, 100, 1_000, 1_020_000, 990);

        let mut changed = fixture.transaction.clone();
        changed.message.instructions[0].data[0] = 0;
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        changed.message.instructions[1].accounts[1] = 1;
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());
    }

    #[test]
    fn accepts_post_skew_estimate_distinct_from_raw_quote() {
        let mut fixture = fixture(RfqSide::Sell, 100, 1_000, 1_020_000, 990);
        fixture.request.estimated_out = 950;
        fixture.request.estimated_quote.quote_amount = 950;
        fixture.request.estimated_quote.adjustment_fp = 950_000;
        fixture.transaction.message.instructions[0].data[138..146]
            .copy_from_slice(&940u64.to_le_bytes());

        let quote = validate_quote_transaction(&fixture.transaction, &fixture.request).unwrap();

        assert_eq!(quote.estimated_out, 950);
    }

    #[test]
    fn rejects_untrusted_transaction_shape_and_signers() {
        let fixture = fixture(RfqSide::Sell, 100, 1_000, 1_020_000, 990);

        let mut changed = fixture.transaction.clone();
        changed.version = 1;
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        changed.signatures.push(arch_sdk::Signature([1; 64]));
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        changed.message.header.num_required_signatures = 1;
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        changed.message.account_keys.swap(0, 1);
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        changed
            .message
            .instructions
            .push(changed.message.instructions[0].clone());
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());
    }

    #[test]
    fn rejects_changed_trade_and_expiry_fields() {
        let valid_fixture = fixture(RfqSide::Sell, 100, 1_000, 1_020_000, 990);

        for offset in [1usize, 33, 65, 66, 74, 82, 138] {
            let mut changed = valid_fixture.transaction.clone();
            changed.message.instructions[0].data[offset] ^= 1;
            assert!(
                validate_quote_transaction(&changed, &valid_fixture.request).is_err(),
                "changed wire byte at offset {offset} was accepted"
            );
        }

        let expired = fixture(RfqSide::Sell, 100, 1_000, 1_002_999, 990);
        assert!(validate_quote_transaction(&expired.transaction, &expired.request).is_err());
    }

    #[test]
    fn rejects_program_account_and_permission_changes() {
        let fixture = fixture(RfqSide::Sell, 100, 1_000, 1_020_000, 990);

        let mut changed = fixture.transaction.clone();
        let program_index = changed.message.instructions[0].program_id_index as usize;
        changed.message.account_keys[program_index] = key(77);
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        let account_index = changed.message.instructions[0].accounts[0] as usize;
        changed.message.account_keys[account_index] = key(78);
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());

        let mut changed = fixture.transaction.clone();
        changed.message.header.num_readonly_unsigned_accounts = 2;
        assert!(validate_quote_transaction(&changed, &fixture.request).is_err());
    }

    #[tokio::test]
    async fn execute_retries_identical_body_with_only_the_user_signature() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let (base_url, server) = spawn_swap_server(received.clone(), true).await;
        let client = test_client(base_url);
        let (liquidator, liquidator_pubkey, _) = generate_new_keypair(Network::Testnet);
        let message = ArchMessage::new(&[], Some(liquidator_pubkey), Hash::from([3; 32]));
        let original_message = message.clone();
        let expected_hash = Hash::from([7; 32]);
        let quote = RfqQuote {
            transaction: RuntimeTransaction {
                version: 0,
                signatures: Vec::new(),
                message,
            },
            side: RfqSide::Sell,
            amount_in: 100,
            estimated_out: 1_000,
            expiry_ts: unix_time_ms().unwrap() + 20_000,
        };

        let actual_hash = client
            .execute_quote(quote, &liquidator, Network::Testnet)
            .await
            .unwrap();

        assert_eq!(actual_hash, expected_hash);
        let bodies = received.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
        let submitted: RuntimeTransaction = serde_json::from_slice(&bodies[0]).unwrap();
        assert_eq!(submitted.message, original_message);
        assert_eq!(submitted.signatures.len(), 1);
        assert_ne!(submitted.signatures[0].to_array(), [0; 64]);
        server.abort();
    }

    #[tokio::test]
    async fn execute_does_not_retry_structured_fatal_server_error() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let (base_url, server) = spawn_swap_server(received.clone(), false).await;
        let client = test_client(base_url);
        let (liquidator, liquidator_pubkey, _) = generate_new_keypair(Network::Testnet);
        let quote = RfqQuote {
            transaction: RuntimeTransaction {
                version: 0,
                signatures: Vec::new(),
                message: ArchMessage::new(&[], Some(liquidator_pubkey), Hash::from([3; 32])),
            },
            side: RfqSide::Sell,
            amount_in: 100,
            estimated_out: 1_000,
            expiry_ts: unix_time_ms().unwrap() + 20_000,
        };

        let error = client
            .execute_quote(quote, &liquidator, Network::Testnet)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("rfq_quote_expired"));
        assert_eq!(received.lock().unwrap().len(), 1);
        server.abort();
    }

    #[derive(Clone)]
    struct SwapServerState {
        received: Arc<Mutex<Vec<Vec<u8>>>>,
        retry_once: bool,
    }

    async fn spawn_swap_server(
        received: Arc<Mutex<Vec<Vec<u8>>>>,
        retry_once: bool,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/rfq/swap", post(swap_handler))
            .with_state(SwapServerState {
                received,
                retry_once,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), server)
    }

    async fn swap_handler(State(state): State<SwapServerState>, body: Bytes) -> Response {
        let request_number = {
            let mut received = state.received.lock().unwrap();
            received.push(body.to_vec());
            received.len()
        };
        if state.retry_once && request_number == 1 {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "submission already processing",
                    "code": "rfq_swap_in_progress"
                })),
            )
                .into_response();
        }
        if !state.retry_once {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "quote expired",
                    "code": "rfq_quote_expired"
                })),
            )
                .into_response();
        }
        (
            StatusCode::OK,
            Json(json!({ "transaction_hash": Hash::from([7; 32]).to_string() })),
        )
            .into_response()
    }

    fn test_client(base_url: String) -> PropAmmClient {
        PropAmmClient::new(&PropAmmConfig {
            base_url,
            expected_program_id: hex::encode(key(1).serialize()),
            slippage_bps: 100,
            request_timeout_ms: 1_000,
            minimum_expiry_headroom_ms: 1_000,
        })
        .unwrap()
    }

    struct Fixture {
        transaction: RuntimeTransaction,
        request: ValidationRequest,
    }

    fn fixture(
        side: RfqSide,
        amount_in: u64,
        estimated_out: u64,
        expiry_ts: u128,
        min_out: u64,
    ) -> Fixture {
        fixture_inner(side, amount_in, estimated_out, expiry_ts, min_out, false)
    }

    fn fixture_with_ata_prelude(
        side: RfqSide,
        amount_in: u64,
        estimated_out: u64,
        expiry_ts: u128,
        min_out: u64,
    ) -> Fixture {
        fixture_inner(side, amount_in, estimated_out, expiry_ts, min_out, true)
    }

    fn fixture_inner(
        side: RfqSide,
        amount_in: u64,
        estimated_out: u64,
        expiry_ts: u128,
        min_out: u64,
        include_ata_prelude: bool,
    ) -> Fixture {
        let service = ServiceMetadata {
            program_id: key(1),
            config_pubkey: derive(&key(1), &[b"config", key(2).as_ref()]),
            quote_signer: key(2),
            max_quote_ttl_ms: 30_000,
        };
        let market = market();
        let user = key(3);
        let (base_amount, quote_amount) = match side {
            RfqSide::Buy => (estimated_out, amount_in),
            RfqSide::Sell => (amount_in, estimated_out),
        };
        let data = execute_trade_data(
            market.base_mint,
            market.quote_mint,
            side,
            base_amount,
            quote_amount,
            user,
            expiry_ts,
            42,
            min_out,
        );
        let accounts = vec![
            AccountMeta::new(service.config_pubkey, false),
            AccountMeta::new(service.quote_signer, true),
            AccountMeta::new(user, true),
            AccountMeta::new(
                derive(
                    &service.program_id,
                    &[b"user_nonce", service.config_pubkey.as_ref(), user.as_ref()],
                ),
                false,
            ),
            AccountMeta::new(market.base_vault, false),
            AccountMeta::new(market.quote_vault, false),
            AccountMeta::new(
                autara_lib::token::get_associated_token_address(&user, &market.base_mint),
                false,
            ),
            AccountMeta::new(
                autara_lib::token::get_associated_token_address(&user, &market.quote_mint),
                false,
            ),
            AccountMeta::new(market.base_mint, false),
            AccountMeta::new(market.quote_mint, false),
            AccountMeta::new(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(apl_token::id(), false),
        ];
        let instruction = Instruction {
            program_id: service.program_id,
            accounts,
            data,
        };
        let mut instructions = Vec::new();
        if include_ata_prelude {
            instructions.push(autara_lib::token::create_ata_ix(
                &user,
                None,
                &user,
                &market.base_mint,
            ));
            instructions.push(autara_lib::token::create_ata_ix(
                &user,
                None,
                &user,
                &market.quote_mint,
            ));
        }
        instructions.push(instruction);
        let transaction = RuntimeTransaction {
            version: 0,
            signatures: Vec::new(),
            message: ArchMessage::new(&instructions, Some(user), Hash::from([9; 32])),
        };
        let estimated_quote = EstimatedQuote {
            base_amount,
            quote_amount,
            adjustment_fp: 1_000_000,
            vault_observed_at_ms: 123,
        };
        Fixture {
            transaction,
            request: ValidationRequest {
                service,
                market,
                user,
                side,
                amount_in,
                estimated_out,
                estimated_quote,
                slippage_bps: 100,
                minimum_expiry_headroom_ms: 3_000,
                now_ms: 1_000_000,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_trade_data(
        base_mint: Pubkey,
        quote_mint: Pubkey,
        side: RfqSide,
        base_amount: u64,
        quote_amount: u64,
        user: Pubkey,
        expiry_ts: u128,
        nonce: u64,
        min_out: u64,
    ) -> Vec<u8> {
        let mut data = vec![2]; // PropAmmInstruction::ExecuteTrade
        data.extend_from_slice(base_mint.as_ref());
        data.extend_from_slice(quote_mint.as_ref());
        data.push(match side {
            RfqSide::Buy => 0,
            RfqSide::Sell => 1,
        });
        data.extend_from_slice(&base_amount.to_le_bytes());
        data.extend_from_slice(&quote_amount.to_le_bytes());
        data.extend_from_slice(user.as_ref());
        data.extend_from_slice(&expiry_ts.to_le_bytes());
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        assert_eq!(data.len(), 146);
        data
    }

    fn market() -> MarketMetadata {
        MarketMetadata {
            base_mint: key(10),
            quote_mint: key(11),
            base_vault: key(12),
            quote_vault: key(13),
        }
    }

    fn estimate() -> EstimatedQuote {
        EstimatedQuote {
            base_amount: 10,
            quote_amount: 100,
            adjustment_fp: 1_000_000,
            vault_observed_at_ms: 1,
        }
    }

    fn key(byte: u8) -> Pubkey {
        Pubkey::from_slice(&[byte; 32])
    }

    fn derive(program_id: &Pubkey, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::try_find_program_address(seeds, program_id)
            .unwrap()
            .0
    }
}
