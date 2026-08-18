use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use arch_sdk::arch_program::hash::Hash;
use arch_sdk::AsyncArchRpcClient;

pub struct BlockhashCache {
    latest: Arc<RwLock<Hash>>,
    job: tokio::task::JoinHandle<()>,
}

impl BlockhashCache {
    /// defaults to 3s, min 1s
    pub async fn new(
        arch_client: AsyncArchRpcClient,
        interval: Option<Duration>,
    ) -> Result<Self, arch_sdk::ArchError> {
        let latest = Arc::new(RwLock::new(arch_client.get_best_block_hash().await?));
        let latest_clone = latest.clone();
        let interval = interval
            .unwrap_or(Duration::from_secs(3))
            .max(Duration::from_secs(1));
        let job = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                match arch_client.get_best_block_hash().await {
                    Ok(blockhash) => {
                        *latest_clone.write().unwrap() = blockhash;
                    }
                    Err(e) => {
                        tracing::error!("Error fetching blockhash: {:?}", e);
                    }
                }
            }
        });
        Ok(BlockhashCache { latest, job })
    }

    pub fn get_blockhash(&self) -> Hash {
        *self.latest.read().unwrap()
    }
}

impl Drop for BlockhashCache {
    fn drop(&mut self) {
        self.job.abort();
    }
}
