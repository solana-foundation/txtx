use txtx_addon_kit::types::cloud_interface::CloudServiceContext;
use txtx_addon_kit::types::commands::{CommandExecutionFutureResult, PreCommandSpecification};
use txtx_addon_kit::types::frontend::LogDispatcher;
use txtx_addon_kit::types::frontend::{Actions, BlockEvent};
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::types::RunbookSupervisionContext;
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::types::{
    commands::{CommandExecutionResult, CommandImplementation, CommandSpecification},
    diagnostics::Diagnostic,
    types::Type,
};
use txtx_addon_kit::uuid::Uuid;

use crate::constants::{DEFAULT_CONFIRMATIONS_NUMBER, RPC_API_URL};

lazy_static! {
    pub static ref CHECK_CONFIRMATIONS: PreCommandSpecification = define_command! {
        CheckEvmConfirmations => {
            name: "Check Transaction Confirmations",
            matcher: "check_confirmations",
            documentation: "The `evm::check_confirmations` action polls the network until the provided `tx_hash` has been confirmed by `confirmations` blocks.",
            implements_signing_capability: false,
            implements_background_task_capability: true,
            inputs: [
                tx_hash: {
                    documentation: "The transaction hash to check.",
                    typing: Type::buffer(),
                    optional: false,
                    tainting: true,
                    internal: false
                },
                rpc_api_url: {
                    documentation: "The URL of the EVM API used to poll for the transaction's inclusion in a block.",
                    typing: Type::string(),
                    optional: false,
                    tainting: false,
                    internal: false
                },
                chain_id: {
                    documentation: "The chain ID of the network to check the transaction on.",
                    typing: Type::integer(),
                    optional: false,
                    tainting: true,
                    internal: false
                },
                confirmations: {
                    documentation: "Once the transaction is included on a block, the number of blocks to await before the transaction is considered successful and Runbook execution continues. The default is 1.",
                    typing: Type::integer(),
                    optional: true,
                    tainting: false,
                    internal: false
                }
            ],
            outputs: [
                contract_address: {
                    documentation: "The contract address from the transaction receipt.",
                    typing: Type::buffer()
                },
                logs: {
                    documentation: "The decoded contract logs from the transaction receipt.",
                    typing: Type::array(Type::array(Type::string()))
                }
            ],
            example: txtx_addon_kit::indoc! {r#"
            action "confirm_deployment" "evm::check_confirmations" {
                tx_hash = action.some_deploying_action.tx_hash
            }
        "#},
        }
    };
}
pub struct CheckEvmConfirmations;
impl CommandImplementation for CheckEvmConfirmations {
    fn check_instantiability(
        _ctx: &CommandSpecification,
        _args: Vec<Type>,
    ) -> Result<Type, Diagnostic> {
        //    Todo: check network consistency?
        // let network = match transaction.version {
        //     TransactionVersion::Mainnet => "mainnet".to_string(),
        //     TransactionVersion::Testnet => "testnet".to_string(),
        // };

        // let network_id = args.get("network_id")
        //     .and_then(|a| Some(a.expect_string()))
        //     .or(defaults.keys.get("network_id").map(|x| x.as_str()))
        //     .ok_or(Diagnostic::error_from_string(format!("Key 'network_id' is missing")))?;
        unimplemented!()
    }

    fn check_executability(
        _construct_id: &ConstructDid,
        _instance_name: &str,
        _spec: &CommandSpecification,
        _values: &ValueStore,
        _supervision_context: &RunbookSupervisionContext,
        _auth_context: &txtx_addon_kit::types::AuthorizationContext,
    ) -> Result<Actions, Diagnostic> {
        Ok(Actions::none())
    }

    #[cfg(not(feature = "wasm"))]
    fn run_execution(
        _construct_id: &ConstructDid,
        _spec: &CommandSpecification,
        _values: &ValueStore,
        _progress_tx: &txtx_addon_kit::channel::Sender<BlockEvent>,
        _auth_ctx: &txtx_addon_kit::types::AuthorizationContext,
    ) -> CommandExecutionFutureResult {
        let future = async move {
            let result = CommandExecutionResult::new();
            Ok(result)
        };

        Ok(Box::pin(future))
    }

    #[cfg(not(feature = "wasm"))]
    fn build_background_task(
        construct_did: &ConstructDid,
        _spec: &CommandSpecification,
        inputs: &ValueStore,
        _outputs: &ValueStore,
        progress_tx: &txtx_addon_kit::channel::Sender<BlockEvent>,
        _background_tasks_uuid: &Uuid,
        _supervision_context: &RunbookSupervisionContext,
        _cloud_service_context: &Option<CloudServiceContext>,
    ) -> CommandExecutionFutureResult {
        use alloy_chains::{Chain, ChainKind};
        use txtx_addon_kit::{
            hex,
            types::{commands::return_synchronous_result, types::Value},
        };

        use crate::{
            codec::abi_decode_logs,
            constants::{
                ADDRESS_ABI_MAP, ALREADY_DEPLOYED, CHAIN_ID, CONTRACT_ADDRESS, LOGS, RAW_LOGS,
                TX_HASH,
            },
            rpc::EvmRpc,
            typing::{EvmValue, RawLog},
        };

        let inputs = inputs.clone();
        let confirmations_required = inputs
            .get_expected_uint("confirmations")
            .unwrap_or(DEFAULT_CONFIRMATIONS_NUMBER) as usize;
        let chain_id = inputs.get_expected_uint(CHAIN_ID)?;
        let chain_name = match Chain::from(chain_id).into_kind() {
            ChainKind::Named(name) => name.to_string(),
            ChainKind::Id(id) => id.to_string(),
        };
        let address_abi_map = inputs.get_value(ADDRESS_ABI_MAP).cloned();
        let progress_tx = progress_tx.clone();
        let logger =
            LogDispatcher::new(construct_did.as_uuid(), "evm::check_confirmations", &progress_tx);

        let skip_confirmations = inputs.get_bool(ALREADY_DEPLOYED).unwrap_or(false);
        let contract_address = inputs.get_value(CONTRACT_ADDRESS).cloned();
        if skip_confirmations {
            let mut result = CommandExecutionResult::new();
            if let Some(contract_address) = contract_address.clone() {
                result.outputs.insert(CONTRACT_ADDRESS.to_string(), contract_address);
            }

            logger.success_info(
                "Confirmed",
                format!(
                    "Contract deployment transaction already confirmed on Chain {}",
                    chain_name
                ),
            );
            return return_synchronous_result(Ok(result));
        }

        let tx_hash_bytes = inputs.get_expected_buffer_bytes(TX_HASH)?;
        let rpc_api_url = inputs.get_expected_string(RPC_API_URL)?.to_owned();

        let tx_hash = hex::encode(&tx_hash_bytes);
        let receipt_msg = format!("Checking Tx 0x{} Receipt on Chain {}", &tx_hash, chain_name);

        let future = async move {
            // initial progress status
            logger.pending_info("Pending", receipt_msg);

            let mut result = CommandExecutionResult::new();

            let backoff_ms = 500;

            let rpc = EvmRpc::new(&rpc_api_url).map_err(|e| diagnosed_error!("{e}"))?;

            let mut tx_inclusion_block = u64::MAX - confirmations_required as u64;
            let mut current_block = 0;
            let mut previous_block = 0;
            let _receipt = loop {
                let Some(receipt) = rpc.get_receipt(&tx_hash_bytes).await.map_err(|e| {
                    diagnosed_error!("failed to verify transaction {}: {}", tx_hash, e)
                })?
                else {
                    sleep_ms(backoff_ms * 10);
                    continue;
                };
                let Some(block_number) = receipt.block_number else {
                    logger.pending_info(
                        "Pending",
                        format!(
                            "Awaiting Inclusion in Block for Tx 0x{} on Chain {}",
                            tx_hash, chain_name
                        ),
                    );

                    sleep_ms(backoff_ms);
                    continue;
                };
                if current_block == 0 {
                    tx_inclusion_block = block_number;
                    current_block = block_number;
                    previous_block = block_number;
                }

                if !receipt.status() {
                    let diag = match rpc.get_transaction_return_value(&tx_hash_bytes).await {
                        Ok(return_value) => {
                            diagnosed_error!(
                                "transaction reverted with return value: {}",
                                return_value
                            )
                        }
                        Err(_) => diagnosed_error!("transaction reverted"),
                    };

                    logger.failure_info(
                        "Failed",
                        format!("Transaction Failed for Chain {}", chain_name),
                    );
                    logger.error("Error", diag.to_string());

                    return Err(diag);
                }
                if let Some(contract_address) = receipt.contract_address {
                    result
                        .outputs
                        .insert(CONTRACT_ADDRESS.to_string(), EvmValue::address(&contract_address));
                }
                // a contract deployed via create2 factory won't have the address in the receipt, so pull it from our inputs
                else if let Some(contract_address) = contract_address.clone() {
                    result.outputs.insert(CONTRACT_ADDRESS.to_string(), contract_address);
                };

                let logs = receipt.inner.logs();
                if let Some(abi) = &address_abi_map {
                    let logs = abi_decode_logs(&abi, logs).map_err(|e| diagnosed_error!(" {e}"))?;
                    result.outputs.insert(LOGS.to_string(), Value::array(logs));
                }
                result.outputs.insert(
                    RAW_LOGS.to_string(),
                    Value::array(
                        logs.iter().map(|log| RawLog::to_value(log)).collect::<Vec<Value>>(),
                    ),
                );

                if current_block >= tx_inclusion_block + confirmations_required as u64 {
                    break receipt;
                } else {
                    let block = rpc.get_block_number().await.unwrap_or(current_block);
                    // only send updates when the mined block is actually updated, so we're not spamming with updates every 500ms
                    if previous_block != block {
                        let _ = logger.pending_info(
                            "Pending",
                            format!(
                                "{}/{} blocks confirmed for Tx 0x{} on chain {}",
                                current_block - tx_inclusion_block,
                                confirmations_required,
                                tx_hash,
                                chain_name
                            ),
                        );
                        previous_block = block.clone();
                    }
                    current_block = block;

                    sleep_ms(backoff_ms);
                    continue;
                }
            };

            logger.success_info(
                "Confirmed",
                format!(
                    "Confirmed {} {} for Tx 0x{} on Chain {}",
                    &confirmations_required,
                    if confirmations_required.eq(&1) { "block" } else { "blocks" },
                    tx_hash,
                    chain_name
                ),
            );

            Ok(result)
        };
        Ok(Box::pin(future))
    }
}

pub fn sleep_ms(millis: u64) -> () {
    let t = std::time::Duration::from_millis(millis);
    std::thread::sleep(t);
}
