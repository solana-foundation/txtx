use std::collections::HashMap;
use std::future;

use kaigan::types::RemainderStr;
use solana_client::rpc_client::RpcClient;
use solana_record_service_client::accounts::Class;
use solana_record_service_client::instructions::{
    CreateClassBuilder, FreezeClassBuilder, UpdateClassMetadataBuilder,
};
use solana_record_service_client::programs::SOLANA_RECORD_SERVICE_ID;
use solana_commitment_config::CommitmentConfig;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_sdk_ids::system_program;
use solana_transaction::Transaction;
use txtx_addon_kit::channel;
use txtx_addon_kit::constants::META_DESCRIPTION;
use txtx_addon_kit::types::cloud_interface::CloudServiceContext;
use txtx_addon_kit::types::commands::{
    CommandExecutionFutureResult, CommandExecutionResult, CommandImplementation,
    CommandSpecification, PreCommandSpecification,
};
use txtx_addon_kit::types::diagnostics::Diagnostic;
use txtx_addon_kit::types::frontend::{Actions, BlockEvent, LogDispatcher};
use txtx_addon_kit::types::signers::{
    return_synchronous_actions, SignerActionsFutureResult, SignerInstance, SignerSignFutureResult,
    SignersState,
};
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::types::{RunbookSupervisionContext, Type, Value};
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;

use super::super::sign_transaction::{check_signed_executability, run_signed_execution};
use crate::codec::send_transaction::send_transaction_background_task;
use crate::codec::ui_encode::{
    get_formatted_transaction_meta_description, ix_to_formatted_value,
    message_data_to_formatted_value,
};
use crate::commands::get_signer_did;
use crate::constants::{CHECKED_PUBLIC_KEY, FORMATTED_TRANSACTION, RPC_API_URL, TRANSACTION_BYTES};
use crate::typing::SvmValue;

use super::to_u8_prefix_string;

const CLASS_PREFIX: &[u8] = b"class";

#[derive(Debug, Clone, PartialEq)]
enum ClassAction {
    Freeze,
    UpdateMetadata,
    Create,
    None,
    FreezeAndUpdateMetadata,
}

impl ClassAction {
    fn to_value(&self) -> Value {
        Value::addon(
            match self {
                ClassAction::Freeze => b"freeze".to_vec(),
                ClassAction::UpdateMetadata => b"update_metadata".to_vec(),
                ClassAction::Create => b"create".to_vec(),
                ClassAction::None => b"none".to_vec(),
                ClassAction::FreezeAndUpdateMetadata => b"freeze_and_update_metadata".to_vec(),
            },
            "svm::class_action",
        )
    }
    fn from_value(value: &Value) -> Self {
        let value = value.as_addon_data().expect("Invalid class action value");
        if value.id != "svm::class_action" {
            panic!("Invalid class action value");
        }
        match value.bytes.as_slice() {
            b"freeze" => ClassAction::Freeze,
            b"update_metadata" => ClassAction::UpdateMetadata,
            b"create" => ClassAction::Create,
            b"none" => ClassAction::None,
            b"freeze_and_update_metadata" => ClassAction::FreezeAndUpdateMetadata,
            _ => panic!("Invalid class action"),
        }
    }
}

lazy_static! {
    pub static ref CREATE_CLASS: PreCommandSpecification = {
        let mut command = define_command! {
            ProcessInstructions => {
                name: "Create a Class with the Solana Record Service program",
                matcher: "create_class",
                documentation: "The `svm::create_class` action is coming soon.",
                implements_signing_capability: true,
                implements_background_task_capability: true,
                inputs: [
                    description: {
                        documentation: "A description of the record.",
                        typing: Type::string(),
                        optional: true,
                        tainting: false,
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
                    name: {
                        documentation: "The name of the class. This must be a valid UTF-8 string that is less than 256 bytes long.",
                        typing: Type::string(),
                        optional: false,
                        tainting: true,
                        internal: false,
                        sensitive: false
                    },
                    metadata: {
                        documentation: "The metadata of the class. This must be a valid UTF-8 string.",
                        typing: Type::string(),
                        optional: false,
                        tainting: true,
                        internal: false,
                        sensitive: false
                    },
                    is_permissioned: {
                        documentation: "Whether the class is permissioned. The default is true.",
                        typing: Type::bool(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    is_frozen: {
                        documentation: "Whether the class is frozen. The default is false.",
                        typing: Type::bool(),
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
                    },
                    name: {
                        documentation: "The name of the class.",
                        typing: Type::string()
                    },
                    metadata: {
                        documentation: "The metadata of the class.",
                        typing: Type::string()
                    },
                    public_key: {
                        documentation: "The public key of the created class.",
                        typing: Type::string()
                    }
                ],
                example: txtx_addon_kit::indoc! {r#"
                    action "my_class" "svm::create_class" {
                        name = "my_class"
                        metadata = "metadata string"
                        is_permissioned = true
                        is_frozen = false
                        signer = signer.creator
                    }
                "#},
            }
        };
        if let PreCommandSpecification::Atomic(ref mut spec) = command {
            spec.create_critical_output = Some("public_key".to_string());
        }

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
        mut signers: SignersState,
        auth_context: &txtx_addon_kit::types::AuthorizationContext,
    ) -> SignerActionsFutureResult {
        let signer_did = get_signer_did(args).unwrap();
        let mut signer_state = signers.pop_signer_state(&signer_did).unwrap();

        let rpc_api_url = args
            .get_expected_string(RPC_API_URL)
            .map_err(|diag| (signers.clone(), signer_state.clone(), diag))?
            .to_string();

        let client = RpcClient::new(rpc_api_url);

        let authority = signer_state
            .get_expected_value(CHECKED_PUBLIC_KEY)
            .and_then(|key| SvmValue::to_pubkey(key).map_err(Into::into))
            .map_err(|diag| (signers.clone(), signer_state.clone(), diag))?;

        let name_str = args
            .get_expected_string("name")
            .map_err(|diag| (signers.clone(), signer_state.clone(), diag))?;

        let name = to_u8_prefix_string(name_str).map_err(|diag| {
            (
                signers.clone(),
                signer_state.clone(),
                format!("invalid name '{}' for class: {}", name_str, diag).into(),
            )
        })?;

        let metadata = args
            .get_expected_string("metadata")
            .map(|s| RemainderStr::from(s.to_string()))
            .map_err(|diag| (signers.clone(), signer_state.clone(), diag))?;

        let class_seeds = [CLASS_PREFIX, authority.as_ref(), name_str.as_bytes()];
        let (class, _) = Pubkey::find_program_address(&class_seeds[..], &SOLANA_RECORD_SERVICE_ID);

        let mut is_permissioned = args.get_bool("is_permissioned");

        let is_frozen = args.get_bool("is_frozen").unwrap_or(false);

        // store the signer state so we can store it in our outputs later
        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            "class",
            SvmValue::pubkey(class.to_bytes().to_vec()),
        );
        let mut class_action = ClassAction::None;

        let (instructions, descriptions, formatted_instructions) = if let Ok(Some(account)) =
            client.get_account_with_commitment(&class, CommitmentConfig::default()).map(|a| a.value)
        {
            let existing_class = Class::from_bytes(&account.data)
                    .map_err(|e| (signers.clone(), signer_state.clone(), diagnosed_error!("class PDA '{class}' exists on chain, but the on-chain account is not a valid class: {e}")))?;

            let mut instructions = vec![];
            let mut descriptions = vec![];
            let mut formatted_instructions = vec![];

            // if the user is changing the `is_frozen` flag, push that instruction
            if existing_class.is_frozen != is_frozen {
                class_action = ClassAction::Freeze;
                let ix = FreezeClassBuilder::new()
                    .authority(authority)
                    .class(class)
                    .is_frozen(is_frozen)
                    .instruction();
                let formatted_ix = ix_to_formatted_value(&ix);
                instructions.push(ix);
                descriptions.push(format!(
                    "Instruction #{} will freeze the '{}' class at address '{}'.",
                    instructions.len(),
                    name_str,
                    class.to_string()
                ));
                formatted_instructions.push(formatted_ix);
            }

            // if the user set the is_permissioned flag, we need to check if it matches the existing class
            // if it doesn't, we need to return an error
            // if it does, we can just use the existing class value as the default
            if let Some(inner_is_permissioned) = is_permissioned {
                if existing_class.is_permissioned != inner_is_permissioned {
                    return Err((
                        signers.clone(),
                        signer_state.clone(),
                        diagnosed_error!(
                            "class PDA '{class}' exists on chain, and the supplied 'is_permissioned' value does not match the existing class; only the 'metadata' and 'frozen' fields can be updated"
                        ),
                    ));
                } else {
                    is_permissioned = Some(existing_class.is_permissioned);
                }
            }

            // if the user is changing the metadata, push that instruction
            if !existing_class.metadata.eq(&metadata) {
                if ClassAction::Freeze == class_action {
                    class_action = ClassAction::FreezeAndUpdateMetadata;
                } else {
                    class_action = ClassAction::UpdateMetadata;
                }
                let ix = UpdateClassMetadataBuilder::new()
                    .authority(authority)
                    .class(class)
                    .system_program(system_program::ID)
                    .metadata(metadata.clone())
                    .instruction();
                let formatted_ix = ix_to_formatted_value(&ix);
                instructions.push(ix);
                descriptions.push(format!(
                    "Instruction #{} will update the '{}' class metadata at address '{}'.",
                    instructions.len(),
                    name_str,
                    class.to_string()
                ));
                formatted_instructions.push(formatted_ix);
            }

            if !instructions.is_empty() && existing_class.is_frozen {
                // todo: can you unfreeze a class? if so, this needs to change
                // currently, if we have some instructions updating our class, we throw if the existing class is frozen
                return Err((
                        signers.clone(),
                        signer_state.clone(),
                        diagnosed_error!(
                            "class PDA '{class}' exists on chain, but the class is frozen so changes cannot be made"
                        ),
                    ));
            }

            (instructions, descriptions, formatted_instructions)
        } else {
            class_action = ClassAction::Create;
            let ix = CreateClassBuilder::new()
                .authority(authority)
                .class(class)
                .system_program(system_program::ID)
                .is_permissioned(is_permissioned.unwrap_or(true))
                .is_frozen(is_frozen)
                .metadata(metadata.clone())
                .name(name.clone())
                .instruction();
            let formatted_ix = ix_to_formatted_value(&ix);
            (
                vec![ix],
                vec![format!(
                    "Instruction #1 will create the '{}' class at address '{}'.",
                    name_str,
                    class.to_string()
                )],
                vec![formatted_ix],
            )
        };

        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            "class_action",
            class_action.to_value(),
        );

        if !instructions.is_empty() {
            let mut message = Message::new(&instructions, None);
            message.recent_blockhash = client.get_latest_blockhash().map_err(|e| {
                (
                    signers.clone(),
                    signer_state.clone(),
                    diagnosed_error!("failed to get latest blockhash: {e}"),
                )
            })?;

            let formatted_transaction = message_data_to_formatted_value(
                &formatted_instructions,
                message.header.num_required_signatures,
                message.header.num_readonly_signed_accounts,
                message.header.num_readonly_unsigned_accounts,
            );

            let transaction = SvmValue::transaction(&Transaction::new_unsigned(message))
                .map_err(|e| (signers.clone(), signer_state.clone(), e))?;

            let meta_description = get_formatted_transaction_meta_description(
                &descriptions,
                &vec![signer_did],
                signers_instances,
            );

            let mut args = args.clone();
            args.insert(TRANSACTION_BYTES, transaction);
            args.insert(FORMATTED_TRANSACTION, formatted_transaction);
            args.insert(META_DESCRIPTION, Value::string(meta_description));

            signers.push_signer_state(signer_state);
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
        } else {
            return_synchronous_actions(Ok((signers, signer_state, Actions::none())))
        }
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
        let signers = signers.clone();
        let construct_did = construct_did.clone();

        let future = async move {
            let signer_did = get_signer_did(&args).unwrap();
            let signer_state = signers.get_signer_state(&signer_did).unwrap().clone();

            let class_action = signer_state
                .get_scoped_value(&construct_did.to_string(), "class_action")
                .map(|v| ClassAction::from_value(v))
                .unwrap();

            let (signers, signer_state, mut result) = if ClassAction::None != class_action {
                let run_signing_future =
                    run_signed_execution(&construct_did, &args, &signers_instances, signers);
                match run_signing_future {
                    Ok(future) => match future.await {
                        Ok(res) => res,
                        Err(err) => return Err(err),
                    },
                    Err(err) => return Err(err),
                }
            } else {
                (signers, signer_state, CommandExecutionResult::new())
            };

            result.insert(
                "class",
                signer_state.get_scoped_value(&construct_did.to_string(), "class").unwrap().clone(),
            );

            result.insert("class_action", class_action.to_value());

            Ok((signers, signer_state, result))
        };
        Ok(Box::pin(future))
    }

    fn build_background_task(
        construct_did: &ConstructDid,
        spec: &CommandSpecification,
        inputs: &ValueStore,
        outputs: &ValueStore,
        progress_tx: &channel::Sender<BlockEvent>,
        _background_tasks_uuid: &Uuid,
        supervision_context: &RunbookSupervisionContext,
        _cloud_service_context: &Option<CloudServiceContext>,
    ) -> CommandExecutionFutureResult {
        let construct_did = construct_did.clone();
        let spec = spec.clone();
        let inputs = inputs.clone();
        let outputs = outputs.clone();
        let progress_tx = progress_tx.clone();
        let supervision_context = supervision_context.clone();

        let future = async move {
            let name = inputs.get_value("name").unwrap();
            let metadata = inputs.get_value("metadata").unwrap();
            let class = outputs.get_value("class").unwrap();
            let class_action = ClassAction::from_value(inputs.get_value("class_action").unwrap());

            let logger =
                LogDispatcher::new(construct_did.as_uuid(), "svm::create_class", &progress_tx);

            let mut result = if class_action != ClassAction::None {
                match send_transaction_background_task(
                    &construct_did,
                    &spec,
                    &inputs,
                    &outputs,
                    &progress_tx,
                    &supervision_context,
                ) {
                    Ok(res) => match res.await {
                        Ok(res) => res,
                        Err(e) => return Err(e),
                    },
                    Err(e) => return Err(e),
                }
            } else {
                CommandExecutionResult::new()
            };

            match class_action {
                ClassAction::Freeze => {
                    logger.success_info(
                        "Class Frozen",
                        format!("Class {} frozen", name.as_string().unwrap()),
                    );
                }
                ClassAction::UpdateMetadata => {
                    logger.success_info(
                        "Class Updated",
                        format!("Class {} metadata updated", name.as_string().unwrap()),
                    );
                }
                ClassAction::Create => {
                    logger.success_info(
                        "Class Created",
                        format!("Class {} created", name.as_string().unwrap()),
                    );
                }
                ClassAction::FreezeAndUpdateMetadata => {
                    logger.success_info(
                        "Class Updated",
                        format!("Class {} frozen and metadata updated", name.as_string().unwrap()),
                    );
                }
                ClassAction::None => {
                    logger.success_info(
                        "Class Unchanged",
                        format!(
                            "Class {} already exists on chain, and no changes were applied",
                            name.as_string().unwrap()
                        ),
                    );
                }
            }

            result.insert("name", name.clone());
            result.insert("metadata", metadata.clone());
            result.insert("public_key", class.clone());
            Ok(result)
        };
        Ok(Box::pin(future))
    }
}
