use std::collections::HashSet;

use anyhow::{Context, Result};
use arch_sdk::arch_program::pubkey::Pubkey;
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
    /// Whirlpools config address (hex). If omitted, uses the default.
    pub whirlpools_config: Option<String>,
    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Optional set of token addresses (hex) to restrict scanning to.
    /// Only markets whose supply or collateral token is in this set will be considered.
    /// If omitted or empty, all markets are scanned.
    #[serde(default)]
    pub restrict_tokens: Vec<String>,
}

fn default_poll_interval() -> u64 {
    5
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

    /// Returns true if the given token is allowed (or if no filter is active).
    pub fn allows_token(&self, token: &Pubkey) -> bool {
        self.tokens.is_empty() || self.tokens.contains(token)
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
