use std::collections::HashMap;
use std::str::FromStr;

use solana_client::rpc_client::RpcClient;
use solana_message::Message;
use solana_pubkey::Pubkey;
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
use txtx_addon_kit::types::types::{RunbookSupervisionContext, Type};
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;

use crate::codec::send_transaction::send_transaction_background_task;
use crate::commands::sign_transaction::check_signed_executability;
use crate::constants::{AMOUNT, CHECKED_PUBLIC_KEY, RECIPIENT, RPC_API_URL, TRANSACTION_BYTES};
use crate::typing::SvmValue;

use super::get_signer_did;
use super::sign_transaction::run_signed_execution;

lazy_static! {
    pub static ref SEND_SOL: PreCommandSpecification = define_command! {
        SendSol => {
            name: "Send SOL",
            matcher: "send_sol",
            documentation: "The `svm::send_sol` action encodes a transaction which sends SOL, signs it, and broadcasts it to the network.",
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
                amount: {
                    documentation: "The amount to send, in lamports (1 SOL = 10^9 lamports).",
                    typing: Type::integer(),
                    optional: false,
                    tainting: false,
                    internal: false,
                    sensitive: false
                },
                recipient: {
                    documentation: "The SVM address of the recipient.",
                    typing: Type::string(),
                    optional: false,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                signer: {
                    documentation: "A reference to a signer construct, which will be used to sign the transaction.",
                    typing: Type::array(Type::string()),
                    optional: false,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                commitment_level: {
                    documentation: "The commitment level expected for considering this action as done ('processed', 'confirmed', 'finalized'). The default is 'confirmed'.",
                    typing: Type::string(),
                    optional: true,
                    tainting: false,
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
                }
            ],
            outputs: [
                signature: {
                    documentation: "The transaction computed signature.",
                    typing: Type::string()
                }
            ],
            example: txtx_addon_kit::indoc! {
                r#"action "send_sol" "svm::send_sol" {
                    description = "Send some SOL"
                    amount = svm::sol_to_lamports(1)
                    signer = signer.caller
                    recipient = "zbBjhHwuqyKMmz8ber5oUtJJ3ZV4B6ePmANfGyKzVGV"
                }"#
            },
      }
    };
}

pub struct SendSol;
impl CommandImplementation for SendSol {
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
        let signer_did = get_signer_did(args).unwrap();
        let signer_state = signers.get_signer_state(&signer_did).unwrap();

        let amount = args
            .get_expected_uint(AMOUNT)
            .map_err(|e| (signers.clone(), signer_state.clone(), e))?;

        let recipient = Pubkey::from_str(
            args.get_expected_string(RECIPIENT)
                .map_err(|e| (signers.clone(), signer_state.clone(), e))?,
        )
        .map_err(|e| {
            (
                signers.clone(),
                signer_state.clone(),
                diagnosed_error!("invalid recipient: {}", e.to_string()),
            )
        })?;

        let rpc_api_url = args
            .get_expected_string(RPC_API_URL)
            .map_err(|e| (signers.clone(), signer_state.clone(), e))?
            .to_string();

        let signer_pubkey = signer_state
            .get_expected_value(CHECKED_PUBLIC_KEY)
            .and_then(|v| SvmValue::to_pubkey(v).map_err(Into::into))
            .map_err(|e| (signers.clone(), signer_state.clone(), e))?;

        let instruction =
            solana_system_interface::instruction::transfer(&signer_pubkey, &recipient, amount);

        let mut message = Message::new(&vec![instruction], None);
        let client = RpcClient::new(rpc_api_url);
        message.recent_blockhash = client.get_latest_blockhash().map_err(|e| {
            (
                signers.clone(),
                signer_state.clone(),
                diagnosed_error!("failed to retrieve latest blockhash: {}", e.to_string()),
            )
        })?;

        let transaction = SvmValue::transaction(&Transaction::new_unsigned(message))
            .map_err(|diag| (signers.clone(), signer_state.clone(), diag))?;

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
