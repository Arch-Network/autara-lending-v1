use std::collections::HashSet;

use anyhow::{Context, Result};
use arch_sdk::arch_program::{
    bitcoin::{key::Keypair, Network},
    pubkey::Pubkey,
};
use clap::Parser;
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "autara-liquidator")]
#[command(about = "Liquidator bot for the Autara Lending protocol")]
pub struct Args {
    /// Path to the config file
    #[arg(long, default_value = "liquidator-config.json")]
    pub config: String,
}

#[derive(Debug, Deserialize)]
pub struct LiquidatorConfig {
    /// RPC URL for the Arch node
    pub rpc_url: String,
    /// Autara lending program ID (hex)
    pub autara_program_id: String,
    /// Path to the liquidator keypair file
    pub liquidator_keypair: String,
    /// Whirlpools config address (hex). If omitted, uses the default.
    pub whirlpools_config: Option<String>,
    /// Bitcoin network for signing. One of: "regtest", "testnet", "signet", "bitcoin".
    #[serde(default = "default_network")]
    pub network: String,
    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// If true, the bot will skip broadcasting signed transactions (dry run).
    #[serde(default)]
    pub dry_run: bool,
    /// Optional set of token addresses (hex) to restrict scanning to.
    /// Only markets whose supply or collateral token is in this set will be considered.
    /// If omitted or empty, all markets are scanned.
    #[serde(default)]
    pub restrict_tokens: Vec<String>,
    /// Optional PropAMM (RFQ vault AMM) liquidity venue. When present, the liquidator
    /// quotes both CLAMM and PropAMM per liquidation and routes to the higher output.
    #[serde(default)]
    pub propamm: Option<PropAmmConfig>,
}

/// PropAMM venue configuration (all pubkeys hex). See prop-amm/deployments.testnet.json.
#[derive(Debug, Deserialize)]
pub struct PropAmmConfig {
    pub program_id: String,
    pub config_pubkey: String,
    /// Path to the quote_signer keypair (must co-sign every PropAMM swap).
    pub quote_signer_keypair: String,
    pub base_mint: String,
    pub quote_mint: String,
    pub base_vault: String,
    pub quote_vault: String,
    pub base_decimals: u32,
    pub quote_decimals: u32,
    pub backend_url: String,
}

impl LiquidatorConfig {
    pub fn load_keypair(&self) -> Result<(Keypair, Pubkey)> {
        arch_sdk::with_secret_key_file(&self.liquidator_keypair)
            .context("failed to load liquidator keypair")
    }

    /// Build the PropAMM venue client if configured.
    pub fn build_propamm(&self) -> Result<Option<crate::propamm::PropAmm>> {
        let Some(c) = &self.propamm else {
            return Ok(None);
        };
        let (quote_signer_kp, quote_signer_pk) =
            arch_sdk::with_secret_key_file(&c.quote_signer_keypair)
                .context("failed to load propamm quote_signer keypair")?;
        Ok(Some(crate::propamm::PropAmm {
            program_id: parse_hex_pubkey(&c.program_id)?,
            config_pubkey: parse_hex_pubkey(&c.config_pubkey)?,
            quote_signer_kp,
            quote_signer_pk,
            base_mint: parse_hex_pubkey(&c.base_mint)?,
            quote_mint: parse_hex_pubkey(&c.quote_mint)?,
            base_vault: parse_hex_pubkey(&c.base_vault)?,
            quote_vault: parse_hex_pubkey(&c.quote_vault)?,
            base_decimals: c.base_decimals,
            quote_decimals: c.quote_decimals,
            backend_url: c.backend_url.clone(),
        }))
    }

    pub fn parse_network(&self) -> Result<Network> {
        match self.network.as_str() {
            "regtest" => Ok(Network::Regtest),
            "testnet" | "testnet3" => Ok(Network::Testnet),
            "testnet4" => Ok(Network::Testnet4),
            "signet" => Ok(Network::Signet),
            "bitcoin" | "mainnet" => Ok(Network::Bitcoin),
            other => Err(anyhow::anyhow!("unknown network: {}", other)),
        }
    }
}

fn default_poll_interval() -> u64 {
    5
}

fn default_network() -> String {
    "regtest".to_string()
}

pub fn parse_hex_pubkey(hex_str: &str) -> Result<Pubkey> {
    let bytes = hex::decode(hex_str).context("invalid hex for pubkey")?;
    Ok(Pubkey::from_slice(&bytes))
}

/// Optional token filter that restricts which markets/tokens the liquidator considers.
/// When empty, everything passes. When non-empty, only items involving at least one
/// of the listed tokens are accepted.
#[derive(Debug, Clone)]
pub struct TokenFilter {
    tokens: HashSet<Pubkey>,
}

impl TokenFilter {
    pub fn from_config(hex_list: &[String]) -> Result<Self> {
        let tokens = hex_list
            .iter()
            .map(|hex| parse_hex_pubkey(hex))
            .collect::<Result<_>>()?;
        Ok(Self { tokens })
    }

    /// Returns true if filtering is active (at least one token specified).
    pub fn is_active(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// Returns true if a market with the given supply/collateral mints is allowed.
    /// A market passes if at least one of its mints is in the filter set.
    pub fn allows_market(&self, supply_mint: &Pubkey, collateral_mint: &Pubkey) -> bool {
        self.tokens.is_empty()
            || self.tokens.contains(supply_mint)
            || self.tokens.contains(collateral_mint)
    }

    /// Filter a set of tokens, keeping only those that pass the filter.
    pub fn filter_tokens(&self, tokens: HashSet<Pubkey>) -> HashSet<Pubkey> {
        if self.tokens.is_empty() {
            tokens
        } else {
            tokens.intersection(&self.tokens).copied().collect()
        }
    }
}
