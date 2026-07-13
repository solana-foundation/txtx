use std::sync::Arc;

use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_transaction_rpc::Transaction;
use txtx_addon_kit::channel;
use txtx_addon_kit::constants::SIGNED_TRANSACTION_BYTES;
use txtx_addon_kit::types::commands::CommandExecutionResult;
use txtx_addon_kit::types::commands::{CommandExecutionFutureResult, CommandSpecification};
use txtx_addon_kit::types::diagnostics::Diagnostic;
use txtx_addon_kit::types::frontend::{BlockEvent, LogDispatcher};
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::types::{RunbookSupervisionContext, ThirdPartySignatureStatus, Value};
use txtx_addon_kit::types::ConstructDid;

use crate::constants::{
    COMMITMENT_LEVEL, DO_AWAIT_CONFIRMATION, IS_DEPLOYMENT, RPC_API_URL, SIGNATURE,
};

pub fn send_transaction_background_task(
    construct_did: &ConstructDid,
    _spec: &CommandSpecification,
    inputs: &ValueStore,
    outputs: &ValueStore,
    progress_tx: &channel::Sender<BlockEvent>,
    _supervision_context: &RunbookSupervisionContext,
) -> CommandExecutionFutureResult {
    let outputs = outputs.clone();
    let third_party_signature_status = inputs.get_third_party_signature_status();

    if let Some(ThirdPartySignatureStatus::Approved) = third_party_signature_status {
        return Ok(Box::pin(async move {
            let result = CommandExecutionResult::from_value_store(&outputs);
            Ok(result)
        }));
    }

    let construct_did = construct_did.clone();
    let outputs = outputs.clone();
    let inputs = inputs.clone();
    let progress_tx = progress_tx.clone();
    let construct_did = construct_did.clone();

    let future = async move {
        let rpc_api_url = inputs.get_expected_string(RPC_API_URL).unwrap().to_string();
        let commitment_level = inputs.get_expected_string(COMMITMENT_LEVEL).unwrap_or("confirmed");
        let do_await_confirmation = inputs.get_bool(DO_AWAIT_CONFIRMATION).unwrap_or(true);
        let is_deployment = inputs.get_bool(IS_DEPLOYMENT).unwrap_or(false);

        let signed_transaction_value = if is_deployment {
            inputs.get_value(SIGNED_TRANSACTION_BYTES).unwrap()
        } else {
            outputs.get_value(SIGNED_TRANSACTION_BYTES).unwrap()
        };

        let commitment_config = CommitmentConfig {
            commitment: match commitment_level {
                "processed" => CommitmentLevel::Processed,
                "confirmed" => CommitmentLevel::Confirmed,
                "finalized" => CommitmentLevel::Finalized,
                _ => CommitmentLevel::Processed,
            },
        };

        let client = Arc::new(RpcClient::new_with_commitment(
            rpc_api_url.clone(),
            commitment_config.clone(),
        ));

        let logger =
            LogDispatcher::new(construct_did.as_uuid(), "svm::send_transaction", &progress_tx);

        let mut result = CommandExecutionResult::from_value_store(&outputs);

        let transaction_bytes = signed_transaction_value
            .get_buffer_bytes_result()
            .map_err(|e| diagnosed_error!("{}", e))?;
        let signature = send_transaction(
            client.clone(),
            do_await_confirmation,
            &transaction_bytes,
            commitment_config.commitment,
        )
        .map_err(|diag| {
            logger.failure_with_diag("Failed", "Failed to broadcast transaction", &diag);
            diag
        })?;
        result.outputs.insert(SIGNATURE.into(), Value::string(signature.clone()));

        Ok(result)
    };

    Ok(Box::pin(future))
}

pub fn send_transaction(
    rpc_client: Arc<RpcClient>,
    // rpc_config: &RpcSendTransactionConfig,
    do_await_confirmation: bool,
    transaction_bytes: &Vec<u8>,
    commitment: CommitmentLevel,
) -> Result<String, Diagnostic> {
    let transaction = deserialize_transaction(transaction_bytes)?;

    let signature = if do_await_confirmation {
        rpc_client.send_and_confirm_transaction(&transaction).map_err(|e| {
            diagnosed_error!("unable to send and confirm transaction ({})", e.to_string())
        })?
    } else {
        rpc_client
            .send_transaction_with_config(
                &transaction,
                RpcSendTransactionConfig {
                    skip_preflight: true,
                    preflight_commitment: Some(commitment),
                    encoding: None,
                    max_retries: None,
                    min_context_slot: None,
                },
            )
            .map_err(|e| diagnosed_error!("unable to send transaction ({})", e.to_string()))?
    };

    Ok(signature.to_string())
}

fn deserialize_transaction(transaction_bytes: &[u8]) -> Result<Transaction, Diagnostic> {
    serde_json::from_slice(transaction_bytes).map_err(|e| {
        diagnosed_error!("unable to deserialize transaction from bytes ({})", e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use solana_hash::Hash;
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::Transaction as LegacyTransaction;

    use super::deserialize_transaction;

    #[test]
    fn deserializes_legacy_transaction_for_current_rpc_client() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_from_array([1; 32]);
        let message =
            Message::new(&[transfer(&payer.pubkey(), &recipient, 1)], Some(&payer.pubkey()));
        let legacy_transaction =
            LegacyTransaction::new(&[&payer], message, Hash::new_from_array([2; 32]));
        assert!(legacy_transaction.signatures[0].as_ref().iter().any(|byte| *byte != 0));
        let transaction_bytes = serde_json::to_vec(&legacy_transaction).unwrap();

        let transaction = deserialize_transaction(&transaction_bytes).unwrap();

        assert_eq!(
            serde_json::to_value(transaction).unwrap(),
            serde_json::to_value(legacy_transaction).unwrap()
        );
    }
}
