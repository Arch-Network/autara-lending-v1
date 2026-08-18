//! On-disk record of everything a deploy produced. Written to
//! `deployments/<network>.json` so every branch carries the context of what is
//! live. Contains addresses and tx ids only — never private keys.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct DeploymentArtifact {
    pub network: String,
    pub arch_rpc_url: String,
    pub deployed_at_unix: u64,

    pub program_id: String,
    pub oracle_id: String,
    pub build_commit: Option<String>,

    pub program_elf_path: String,
    pub program_elf_sha256: Option<String>,
    pub oracle_elf_path: String,
    pub oracle_elf_sha256: Option<String>,

    pub deployer: String,
    pub admin: String,
    pub fee_receiver: String,
    pub protocol_fee_share_bps: u16,

    pub global_config: Option<String>,

    pub tokens: Vec<TokenRecord>,

    pub markets: Vec<MarketRecord>,

    pub transactions: Vec<TxRecord>,
}

#[derive(Debug, Serialize)]
pub struct TokenRecord {
    pub label: String,
    pub mint: String,
    pub decimals: u8,
    /// Configured initial-supply mint amount (raw units) for this token.
    pub mint_amount: u64,
    /// Configured server faucet amount (raw units) for this token.
    pub faucet_amount: u64,
}

/// A market created (or already present) during the deploy. Addresses only.
#[derive(Debug, Serialize)]
pub struct MarketRecord {
    pub supply_label: String,
    pub collateral_label: String,
    pub supply_mint: String,
    pub collateral_mint: String,
    pub index: u8,
    pub market: String,
    /// `true` if this run created the market, `false` if it already existed.
    pub created: bool,
}

#[derive(Debug, Serialize)]
pub struct TxRecord {
    pub step: String,
    pub txid: String,
}

impl DeploymentArtifact {
    pub fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    pub fn record_tx(&mut self, step: &str, txid: String) {
        self.transactions.push(TxRecord {
            step: step.to_string(),
            txid,
        });
    }

    pub fn record_market(&mut self, market: MarketRecord) {
        self.markets.push(market);
    }

    pub fn write(&self, path: &str) -> Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating artifact dir {}", parent.display()))?;
            }
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).with_context(|| format!("writing artifact {path}"))?;
        Ok(())
    }
}
