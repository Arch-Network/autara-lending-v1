use std::time::{Duration, Instant};

use arch_program::{bitcoin::Network, pubkey::Pubkey};
use serde_json::json;

const WARNING_BALANCE_LAMPORTS: u64 = 172_800_000;
const CRITICAL_BALANCE_LAMPORTS: u64 = 21_600_000;
const RETRY_INTERVAL: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BalanceLevel {
    Healthy,
    Warning,
    Critical,
}

pub struct SlackBalanceAlerter {
    webhook_url: Option<String>,
    network: Network,
    signer_pubkey: Pubkey,
    last_notified: BalanceLevel,
    last_attempt: Option<Instant>,
    client: reqwest::Client,
}

impl SlackBalanceAlerter {
    pub fn from_env(network: Network, signer_pubkey: Pubkey) -> Self {
        let webhook_url = std::env::var("SLACK_WEBHOOK_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        // Alerts only fire on threshold transitions, so a healthy pusher is
        // silent. Log the configuration at startup to make it verifiable.
        match webhook_url {
            Some(_) => tracing::info!(
                warning_lamports = WARNING_BALANCE_LAMPORTS,
                critical_lamports = CRITICAL_BALANCE_LAMPORTS,
                "Slack balance alerts enabled"
            ),
            None => tracing::warn!("Slack balance alerts disabled (SLACK_WEBHOOK_URL unset)"),
        }
        Self {
            webhook_url,
            network,
            signer_pubkey,
            last_notified: BalanceLevel::Healthy,
            last_attempt: None,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("valid Slack HTTP client"),
        }
    }

    pub async fn observe(&mut self, lamports: u64) {
        let level = balance_level(lamports);
        if self.webhook_url.is_none() || level == self.last_notified || !self.can_attempt() {
            return;
        }

        self.last_attempt = Some(Instant::now());
        match self.send(level, lamports).await {
            Ok(()) => {
                self.last_notified = level;
                self.last_attempt = None;
            }
            Err(error) => tracing::error!(%error, "Failed to send pusher balance alert to Slack"),
        }
    }

    fn can_attempt(&self) -> bool {
        self.last_attempt
            .map(|attempt| attempt.elapsed() >= RETRY_INTERVAL)
            .unwrap_or(true)
    }

    async fn send(&self, level: BalanceLevel, lamports: u64) -> reqwest::Result<()> {
        let webhook_url = self.webhook_url.as_ref().expect("checked before send");
        let response = self
            .client
            .post(webhook_url)
            .json(&json!({ "text": self.message(level, lamports) }))
            .send()
            .await?;
        response.error_for_status()?;
        Ok(())
    }

    fn message(&self, level: BalanceLevel, lamports: u64) -> String {
        let network = self.network.to_string();
        let signer = hex::encode(self.signer_pubkey.serialize());
        match level {
            BalanceLevel::Healthy => format!(
                "✅ Autara oracle pusher balance recovered\nNetwork: {network}\nSigner: `{signer}`\nBalance: {lamports} lamports"
            ),
            BalanceLevel::Warning => format!(
                "⚠️ Autara oracle pusher balance is low (<48h runway)\nNetwork: {network}\nSigner: `{signer}`\nBalance: {lamports} lamports\nRefill before it reaches {CRITICAL_BALANCE_LAMPORTS} lamports."
            ),
            BalanceLevel::Critical => format!(
                "🚨 Autara oracle pusher balance is critically low (<6h runway)\nNetwork: {network}\nSigner: `{signer}`\nBalance: {lamports} lamports\nTop up immediately."
            ),
        }
    }
}

fn balance_level(lamports: u64) -> BalanceLevel {
    if lamports <= CRITICAL_BALANCE_LAMPORTS {
        BalanceLevel::Critical
    } else if lamports < WARNING_BALANCE_LAMPORTS {
        BalanceLevel::Warning
    } else {
        BalanceLevel::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_balance_thresholds() {
        assert_eq!(
            balance_level(WARNING_BALANCE_LAMPORTS),
            BalanceLevel::Healthy
        );
        assert_eq!(
            balance_level(WARNING_BALANCE_LAMPORTS - 1),
            BalanceLevel::Warning
        );
        assert_eq!(
            balance_level(CRITICAL_BALANCE_LAMPORTS + 1),
            BalanceLevel::Warning
        );
        assert_eq!(
            balance_level(CRITICAL_BALANCE_LAMPORTS),
            BalanceLevel::Critical
        );
    }
}
