use arch_sdk::{arch_program::pubkey::Pubkey, ArchRpcClient};

use crate::client::{
    blockhash_cache::BlockhashCache, read::AutaraReadClient, tx_broadcast::AutaraTxBroadcast,
    tx_builder::AutaraTransactionBuilder,
};

pub struct AutaraFullClientWithoutSigner<T: AutaraReadClient> {
    read_client: T,
    arch_client: ArchRpcClient,
    blockhash_cache: BlockhashCache,
}

impl<T: AutaraReadClient> AutaraFullClientWithoutSigner<T> {
    pub fn new(
        read_client: T,
        arch_client: ArchRpcClient,
        blockhash_cache: BlockhashCache,
    ) -> Self {
        Self {
            read_client,
            blockhash_cache,
            arch_client,
        }
    }

    pub fn async_arch_client(&self) -> &ArchRpcClient {
        &self.arch_client
    }

    pub fn read_client(&self) -> &T {
        &self.read_client
    }

    pub fn tx_builder(&self, authority: &Pubkey) -> AutaraTransactionBuilder<'_, T> {
        AutaraTransactionBuilder {
            arch_client: &self.arch_client,
            autara_read_client: &self.read_client,
            autara_program_id: *self.read_client.autara_program_id(),
            authority_key: *authority,
            blockhash_cache: Some(&self.blockhash_cache),
        }
    }

    pub fn tx_broadcast(&self) -> AutaraTxBroadcast<'_> {
        AutaraTxBroadcast {
            program_id: self.read_client.autara_program_id(),
            arch_client: &self.arch_client,
        }
    }
}
