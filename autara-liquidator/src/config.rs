use std::collections::HashSet;

use anyhow::{Context, Result};
use arch_sdk::arch_program::{
    bitcoin::{Network, key::Keypair},
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
#[serde(deny_unknown_fields)]
pub struct LiquidatorConfig {
    /// RPC URL for the Arch node
    pub rpc_url: String,
    /// Autara lending program ID (hex)
    pub autara_program_id: String,
    /// Path to the liquidator keypair file
    pub liquidator_keypair: String,
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
    /// Optional CLAMM venue. Program, config, slippage, and pools are all
    /// deployment-specific so one executable can serve every network.
    #[serde(default)]
    pub clamm: Option<ClammConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClammConfig {
    pub program_id: String,
    pub config_pubkey: String,
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps: u16,
    #[serde(default)]
    pub pools: Vec<ClammPoolConfig>,
}

/// An explicit CLAMM whirlpool for a token pair (all pubkeys hex).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClammPoolConfig {
    pub pool: String,
    pub token_a: String,
    pub token_b: String,
}

/// Public-only PropAMM RFQ service configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropAmmConfig {
    pub base_url: String,
    pub expected_program_id: String,
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps: u16,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_minimum_expiry_headroom_ms")]
    pub minimum_expiry_headroom_ms: u64,
}

impl LiquidatorConfig {
    pub fn load_keypair(&self) -> Result<(Keypair, Pubkey)> {
        arch_sdk::with_secret_key_file(&self.liquidator_keypair)
            .context("failed to load liquidator keypair")
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

fn default_slippage_bps() -> u16 {
    100
}

fn default_request_timeout_ms() -> u64 {
    8_000
}

fn default_minimum_expiry_headroom_ms() -> u64 {
    3_000
}

pub fn parse_hex_pubkey(hex_str: &str) -> Result<Pubkey> {
    let value = hex_str.trim();
    if let Ok(bytes) = hex::decode(value) {
        if let Ok(bytes) = <Vec<u8> as TryInto<[u8; 32]>>::try_into(bytes) {
            return Ok(Pubkey::from(bytes));
        }
    }
    if let Ok(bytes) = bs58::decode(value).into_vec() {
        if let Ok(bytes) = <Vec<u8> as TryInto<[u8; 32]>>::try_into(bytes) {
            return Ok(Pubkey::from(bytes));
        }
    }
    Err(anyhow::anyhow!("invalid hex or base58 pubkey"))
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

#[cfg(test)]
mod tests {
    use super::{LiquidatorConfig, PropAmmConfig, parse_hex_pubkey};

    const PROGRAM_ID: &str = "7a68831501d3a9806feff162e82815a36e1732964a2edd2b461faf69575c3628";

    #[test]
    fn public_rfq_config_uses_safe_defaults() {
        let json = format!(
            r#"{{
                "base_url": "https://propamm.arch.network/testnet",
                "expected_program_id": "{PROGRAM_ID}"
            }}"#
        );

        let config: PropAmmConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.slippage_bps, 100);
        assert_eq!(config.request_timeout_ms, 8_000);
        assert_eq!(config.minimum_expiry_headroom_ms, 3_000);
    }

    #[test]
    fn public_rfq_config_rejects_quote_signer_secret() {
        let json = format!(
            r#"{{
                "base_url": "https://propamm.arch.network/testnet",
                "expected_program_id": "{PROGRAM_ID}",
                "__legacy_field__": "keys/propamm-quote-signer.key",
                "program_id": "{PROGRAM_ID}",
                "config_pubkey": "{PROGRAM_ID}",
                "base_mint": "{PROGRAM_ID}",
                "quote_mint": "{PROGRAM_ID}",
                "base_vault": "{PROGRAM_ID}",
                "quote_vault": "{PROGRAM_ID}",
                "base_decimals": 8,
                "quote_decimals": 6,
                "backend_url": "https://propamm.arch.network/testnet"
            }}"#
        )
        .replace("__legacy_field__", concat!("quote_", "signer_keypair"));

        assert!(serde_json::from_str::<PropAmmConfig>(&json).is_err());
    }

    #[test]
    fn top_level_config_rejects_legacy_or_misspelled_deployment_fields() {
        let json = format!(
            r#"{{
                "rpc_url": "https://rpc.testnet.arch.network",
                "autara_program_id": "{PROGRAM_ID}",
                "liquidator_keypair": "/path/to/liquidator.key",
                "network": "testnet",
                "whirlpools_config": "{PROGRAM_ID}"
            }}"#
        );

        assert!(serde_json::from_str::<LiquidatorConfig>(&json).is_err());
    }

    #[test]
    fn pubkey_parser_rejects_wrong_lengths_without_panicking() {
        assert!(parse_hex_pubkey("00").is_err());
        assert!(parse_hex_pubkey(&"00".repeat(33)).is_err());
        assert!(parse_hex_pubkey(PROGRAM_ID).is_ok());
    }

    #[test]
    fn pubkey_parser_accepts_propamm_base58_addresses() {
        let parsed = parse_hex_pubkey("9EqAsENtgBA4Uo4wbS8LVdaQJjMKPpagMxgDVhxEWtKq").unwrap();

        assert_eq!(
            parsed.serialize(),
            [
                0x7a, 0x68, 0x83, 0x15, 0x01, 0xd3, 0xa9, 0x80, 0x6f, 0xef, 0xf1, 0x62, 0xe8, 0x28,
                0x15, 0xa3, 0x6e, 0x17, 0x32, 0x96, 0x4a, 0x2e, 0xdd, 0x2b, 0x46, 0x1f, 0xaf, 0x69,
                0x57, 0x5c, 0x36, 0x28,
            ]
        );
    }

    #[test]
    fn mainnet_and_testnet_examples_use_the_same_schema_and_key_path() {
        let mainnet: LiquidatorConfig =
            serde_json::from_str(include_str!("../liquidator-config.example.json")).unwrap();
        let testnet: LiquidatorConfig =
            serde_json::from_str(include_str!("../liquidator-config.testnet.example.json"))
                .unwrap();

        assert_eq!(mainnet.network, "mainnet");
        assert_eq!(testnet.network, "testnet");
        assert_eq!(mainnet.liquidator_keypair, testnet.liquidator_keypair);
        assert_ne!(mainnet.rpc_url, testnet.rpc_url);
        assert_ne!(
            mainnet.propamm.unwrap().expected_program_id,
            testnet.propamm.unwrap().expected_program_id
        );
        assert_ne!(
            mainnet.clamm.unwrap().config_pubkey,
            testnet.clamm.unwrap().config_pubkey
        );
    }
}
