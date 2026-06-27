use std::collections::HashMap;

use solana_client::rpc_client::RpcClient;
use solana_message::Message;
use solana_transaction::Transaction;
use txtx_addon_kit::channel;
use txtx_addon_kit::futures::future;
use txtx_addon_kit::types::cloud_interface::CloudServiceContext;
use txtx_addon_kit::types::commands::{
    CommandExecutionFutureResult, CommandImplementation, CommandSpecification,
    PreCommandSpecification,
};
use txtx_addon_kit::types::diagnostics::Diagnostic;
use txtx_addon_kit::types::frontend::BlockEvent;
use txtx_addon_kit::types::signers::{
    SignerActionsFutureResult, SignerInstance, SignerSignFutureResult, SignersState,
};
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::types::{RunbookSupervisionContext, Type, Value};
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;

use crate::codec::instruction::parse_instructions_map;
use crate::codec::send_transaction::send_transaction_background_task;
use crate::constants::{DO_AWAIT_CONFIRMATION, RPC_API_URL, SKIP_PREFLIGHT, TRANSACTION_BYTES};
use crate::typing::{SvmValue, INSTRUCTION_TYPE};

use super::get_signers_did;
use super::sign_transaction::{check_signed_executability, run_signed_execution};

lazy_static! {
    pub static ref PROCESS_INSTRUCTIONS: PreCommandSpecification = {
        let command = define_command! {
            ProcessInstructions => {
                name: "Process SVM Instructions",
                matcher: "process_instructions",
                documentation: "The `svm::process_instructions` action encodes instructions, adds them to a transaction, and signs & broadcasts the transaction.",
                implements_signing_capability: true,
                implements_background_task_capability: true,
                inputs: [
                    description: {
                        documentation: "A description of the transaction.",
                        typing: Type::string(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    instruction: {
                        documentation: "The instructions to add to the transaction.",
                        typing: INSTRUCTION_TYPE.clone(),
                        optional: false,
                        tainting: true,
                        internal: false,
                        sensitive: false
                    },
                    signers: {
                        documentation: "A set of references to signer constructs, which will be used to sign the transaction.",
                        typing: Type::array(Type::string()),
                        optional: false,
                        tainting: true,
                        internal: false,
                        sensitive: false
                    },
                    rpc_api_url: {
                        documentation: "The URL to use when making API requests.",
                        typing: Type::string(),
                        optional: false,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    skip_preflight: {
                        documentation: "Whether to skip preflight checks. The default is `false`.",
                        typing: Type::bool(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    }
                ],
                outputs: [
                    signature: {
                        documentation: "The transaction computed signature.",
                        typing: Type::string()
                    }
                ],
                example: txtx_addon_kit::indoc! {r#"
                    action "program_call" "svm::process_instructions" {
                        description = "Invoke instructions"
                        instruction {
                            program_idl = variable.program.idl
                            instruction_name = "initialize"
                            instruction_args = [1]
                            payer {
                                public_key = signer.payer.public_key
                            }
                        }
                        signers = [signer.caller]
                    }
                "#},
            }
        };

        command
    };
}

pub struct ProcessInstructions;
impl CommandImplementation for ProcessInstructions {
    fn check_instantiability(
        _ctx: &CommandSpecification,
        _args: Vec<Type>,
    ) -> Result<Type, Diagnostic> {
        unimplemented!()
    }

    fn check_signed_executability(
        construct_did: &ConstructDid,
        instance_name: &str,
        _spec: &CommandSpecification,
        args: &ValueStore,
        supervision_context: &RunbookSupervisionContext,
        signers_instances: &HashMap<ConstructDid, SignerInstance>,
        signers: SignersState,
        auth_context: &txtx_addon_kit::types::AuthorizationContext,
    ) -> SignerActionsFutureResult {
        let signers_did = get_signers_did(args).unwrap();
        let first_signer_did = signers_did.first().unwrap();
        let first_signer_state = signers.get_signer_state(&first_signer_did).unwrap();

        let rpc_api_url = args
            .get_expected_string(RPC_API_URL)
            .map_err(|diag| (signers.clone(), first_signer_state.clone(), diag))?
            .to_string();

        // TODO: revisit pattern and leverage `check_instantiability` instead`.
        let instructions = parse_instructions_map(args).map_err(|e| {
            (
                signers.clone(),
                first_signer_state.clone(),
                diagnosed_error!("invalid instructions: {e}"),
            )
        })?;

        // Validate at least one writable signer exists (required for fee payer)
        if !has_writable_signer(&instructions) {
            return Err((
                signers.clone(),
                first_signer_state.clone(),
                diagnosed_error!(
                    "no writable signer found in instructions. At least one signer must be \
                     writable to serve as the fee payer. If using an Anchor program, ensure \
                     the signer account has `#[account(mut)]`."
                ),
            ));
        }

        let mut message = Message::new(&instructions, None);
        let client = RpcClient::new(rpc_api_url);
        message.recent_blockhash = client.get_latest_blockhash().map_err(|e| {
            (
                signers.clone(),
                first_signer_state.clone(),
                diagnosed_error!("failed to get latest blockhash: {e}"),
            )
        })?;
        let transaction = SvmValue::transaction(&Transaction::new_unsigned(message))
            .map_err(|e| (signers.clone(), first_signer_state.clone(), e))?;

        let mut args = args.clone();
        args.insert(TRANSACTION_BYTES, transaction);

        let res = check_signed_executability(
            construct_did,
            instance_name,
            &args,
            supervision_context,
            signers_instances,
            signers,
            auth_context,
        );
        Ok(Box::pin(future::ready(res)))
    }

    fn run_signed_execution(
        construct_did: &ConstructDid,
        _spec: &CommandSpecification,
        args: &ValueStore,
        _progress_tx: &channel::Sender<BlockEvent>,
        signers_instances: &HashMap<ConstructDid, SignerInstance>,
        signers: SignersState,
        _auth_context: &txtx_addon_kit::types::AuthorizationContext,
    ) -> SignerSignFutureResult {
        let args = args.clone();
        let signers_instances = signers_instances.clone();
        let construct_did = construct_did.clone();

        let args = args.clone();
        let future = async move {
            let run_signing_future =
                run_signed_execution(&construct_did, &args, &signers_instances, signers);
            let (signers, signer_state, res_signing) = match run_signing_future {
                Ok(future) => match future.await {
                    Ok(res) => res,
                    Err(err) => return Err(err),
                },
                Err(err) => return Err(err),
            };

            Ok((signers, signer_state, res_signing))
        };
        Ok(Box::pin(future))
    }

    fn build_background_task(
        construct_did: &ConstructDid,
        spec: &CommandSpecification,
        values: &ValueStore,
        outputs: &ValueStore,
        progress_tx: &channel::Sender<BlockEvent>,
        _background_tasks_uuid: &Uuid,
        supervision_context: &RunbookSupervisionContext,
        _cloud_service_context: &Option<CloudServiceContext>,
    ) -> CommandExecutionFutureResult {
        let mut values = values.clone();
        let skip_preflight = values.get_bool(SKIP_PREFLIGHT).unwrap_or(false);
        // adding DO_AWAIT_CONFIRMATION skips preflight
        if skip_preflight {
            values.insert(DO_AWAIT_CONFIRMATION, Value::bool(false));
        }
        send_transaction_background_task(
            &construct_did,
            &spec,
            &values,
            &outputs,
            &progress_tx,
            &supervision_context,
        )
    }
}

/// Check if any instruction has at least one account that is both a signer and writable.
/// This is required for fee payer functionality on Solana.
pub fn has_writable_signer(instructions: &[solana_instruction::Instruction]) -> bool {
    instructions.iter().any(|ix| ix.accounts.iter().any(|acc| acc.is_signer && acc.is_writable))
}
