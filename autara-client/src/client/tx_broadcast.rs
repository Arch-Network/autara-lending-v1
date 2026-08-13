use arch_sdk::{
    arch_program::pubkey::Pubkey, AsyncArchRpcClient, ProcessedTransaction, RuntimeTransaction,
    Status,
};
use autara_lib::event::AutaraEvents;
use regex::Regex;

pub struct AutaraTxBroadcast<'a> {
    pub program_id: &'a Pubkey,
    pub arch_client: &'a AsyncArchRpcClient,
}

impl<'a> AutaraTxBroadcast<'a> {
    pub async fn broadcast_transaction(
        &self,
        transaction: RuntimeTransaction,
    ) -> Result<AutaraEvents, AutaraClientError> {
        let sig = hex::encode(transaction.signatures.first().unwrap().0);
        tracing::info!("Sending tx {:?}", sig);
        let tx = self.arch_client.send_transaction(transaction).await?;
        let processed = self.arch_client.wait_for_processed_transaction(&tx).await?;
        parse_processed_autara_tx(processed, self.program_id)
    }
}

pub fn parse_processed_autara_tx(
    processed: ProcessedTransaction,
    program_id: &Pubkey,
) -> Result<AutaraEvents, AutaraClientError> {
    match &processed.status {
        Status::Queued => {
            tracing::info!("Transaction QUEUED, waiting for processing...");
            Err(AutaraClientError::Other(anyhow::anyhow!(
                "Transaction is still queued: {:?}",
                processed
            )))
        }
        Status::Processed => {
            let events = AutaraEvents::from_processed_tx(&processed, |x| x == program_id);
            tracing::info!("Transaction PROCESSED, events = {:?}", events);
            Ok(events)
        }
        Status::Failed(msg) => {
            let re = Regex::new(r"custom program error: 0x([0-9a-fA-F]+)").unwrap();
            let error = re
                .captures(&msg)
                .and_then(|caps| {
                    let hex_str = &caps[1];
                    u32::from_str_radix(hex_str, 16).ok()
                })
                .map(|code| autara_program::error::LendingProgramErrorKind::from_error_code(code));
            tracing::error!("Transaction FAILED, logs = {:?}", processed.logs);
            match error {
                Some(kind) => Err(AutaraClientError::AutaraTxError {
                    kind,
                    events: AutaraEvents::from_processed_tx(&processed, |x| x == program_id),
                    logs: processed.logs,
                }),
                None => Err(AutaraClientError::Other(anyhow::anyhow!(
                    "status = {}, logs = {:?}",
                    msg,
                    processed.logs
                ))),
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AutaraClientError {
    #[error("Autara transaction error: {kind:?}, logs = {logs:?}")]
    AutaraTxError {
        kind: autara_program::error::LendingProgramErrorKind,
        logs: Vec<String>,
        events: AutaraEvents,
    },
    #[error("An error occurred: {0}")]
    Arch(#[from] arch_sdk::ArchError),
    #[error("An error occurred: {0}")]
    Other(#[from] anyhow::Error),
}

impl<I> PartialEq<I> for AutaraClientError
where
    autara_program::error::LendingProgramErrorKind: PartialEq<I>,
{
    fn eq(&self, other: &I) -> bool {
        match self {
            AutaraClientError::AutaraTxError { kind, .. } => kind == other,
            AutaraClientError::Other(_) => false,
            AutaraClientError::Arch(_) => false,
        }
    }
}
