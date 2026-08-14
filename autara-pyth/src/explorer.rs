use std::time::Duration;

use anyhow::{anyhow, Result};
use arch_program::{bitcoin::Network, pubkey::Pubkey};
use serde::Deserialize;

const DEFAULT_EXPLORER_API_URL: &str = "https://explorer.arch.network";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct AccountSummary {
    lamports_balance: Option<i128>,
}

/// Reads signer balances from the indexed explorer API instead of a validator
/// RPC node. Individual RPC nodes can serve stale account state, which flaps
/// the balance alerts between healthy and critical.
pub struct ExplorerClient {
    base_url: String,
    network_segment: &'static str,
    client: reqwest::Client,
}

impl ExplorerClient {
    /// Returns `None` for networks the explorer does not index (regtest and
    /// other local setups), where the caller must fall back to RPC.
    pub fn from_env(network: Network) -> Option<Self> {
        let network_segment = match network {
            Network::Bitcoin => "mainnet",
            Network::Testnet | Network::Testnet4 => "testnet",
            _ => return None,
        };
        let base_url = std::env::var("EXPLORER_API_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_EXPLORER_API_URL.to_string());
        Some(Self {
            base_url,
            network_segment,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("valid explorer HTTP client"),
        })
    }

    pub fn balance_url(&self, pubkey: &Pubkey) -> String {
        format!(
            "{}/api/v1/{}/accounts/{}",
            self.base_url,
            self.network_segment,
            hex::encode(pubkey.serialize())
        )
    }

    pub async fn signer_balance(&self, pubkey: &Pubkey) -> Result<u64> {
        let summary: AccountSummary = self
            .client
            .get(self.balance_url(pubkey))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // The explorer answers 200 with a null/zero balance for accounts it has
        // not indexed, so treat that as unknown rather than an empty signer.
        match summary.lamports_balance {
            Some(lamports) if lamports > 0 => {
                u64::try_from(lamports).map_err(|_| anyhow!("invalid balance {lamports}"))
            }
            _ => Err(anyhow!("explorer reported no indexed balance")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_only_public_networks() {
        assert!(ExplorerClient::from_env(Network::Bitcoin).is_some());
        assert!(ExplorerClient::from_env(Network::Testnet).is_some());
        assert!(ExplorerClient::from_env(Network::Regtest).is_none());
    }

    #[test]
    fn builds_network_scoped_balance_url() {
        let client = ExplorerClient::from_env(Network::Bitcoin).expect("mainnet client");
        let pubkey = Pubkey::from_slice(&[7u8; 32]);
        assert_eq!(
            client.balance_url(&pubkey),
            format!(
                "https://explorer.arch.network/api/v1/mainnet/accounts/{}",
                "07".repeat(32)
            )
        );
    }
}
