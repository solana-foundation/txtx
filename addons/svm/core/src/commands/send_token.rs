use std::collections::{HashMap, VecDeque};
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
use txtx_addon_kit::types::frontend::{BlockEvent, LogDispatcher};
use txtx_addon_kit::types::signers::{
    SignerActionsFutureResult, SignerInstance, SignerSignFutureResult, SignersState,
};
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::types::{RunbookSupervisionContext, Type, Value};
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;

use crate::codec::send_transaction::send_transaction_background_task;
use crate::constants::{
    AMOUNT, AUTHORITY, AUTHORITY_ADDRESS, CHECKED_PUBLIC_KEY, FUND_RECIPIENT, IS_FUNDING_RECIPIENT,
    RECIPIENT, RECIPIENT_ADDRESS, RECIPIENT_TOKEN_ADDRESS, RPC_API_URL, SOURCE_TOKEN_ADDRESS,
    TOKEN, TOKEN_MINT_ADDRESS, TRANSACTION_BYTES,
};
use crate::typing::{SvmValue, SVM_PUBKEY};

use super::get_signers_did;
use super::sign_transaction::{check_signed_executability, run_signed_execution};

lazy_static! {
    pub static ref SEND_TOKEN: PreCommandSpecification = define_command! {
        SendToken => {
            name: "Send Token",
            matcher: "send_token",
            documentation: "The `svm::send_token` action encodes a transaction which sends the specified token, signs it, and broadcasts it to the network.",
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
                    documentation: "The amount of tokens to send, in base unit.",
                    typing: Type::integer(),
                    optional: false,
                    tainting: false,
                    internal: false,
                    sensitive: false
                },
                token: {
                    documentation: "The program address for the token being sent. This is also known as the 'token mint account'.",
                    typing: Type::string(),
                    optional: false,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                recipient: {
                    documentation: "The SVM address of the recipient. The associated token account will be computed from this address and the token address.",
                    typing: Type::string(),
                    optional: false,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                authority: {
                    documentation: "The pubkey of the authority account for the token source. If omitted, the first signer will be used.",
                    typing: Type::string(),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                fund_recipient: {
                    documentation: "If set to `true` and the recipient token account does not exist, the action will create the account and fund it, using the signer to fund the account. The default is `false`.",
                    typing: Type::bool(),
                    optional: true,
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
                }
            ],
            outputs: [
                signature: {
                    documentation: "The transaction computed signature.",
                    typing: Type::string()
                },
                recipient_token_address: {
                    documentation: "The recipient token account address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                source_token_address: {
                    documentation: "The source token account address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                token_mint_address: {
                    documentation: "The token mint address.",
                    typing: Type::addon(SVM_PUBKEY)
                }
            ],
            example: txtx_addon_kit::indoc! {
                r#"action "send_sol" "svm::send_token" {
                    description = "Send some SOL"
                    amount = svm::sol_to_lamports(1)
                    signers = [signer.caller]
                    recipient = "zbBjhHwuqyKMmz8ber5oUtJJ3ZV4B6ePmANfGyKzVGV"
                    token = "3bv3j4GvMPjvvBX9QdoX27pVoWhDSXpwKZipFF1QiVr6"
                    fund_recipient = true
                }"#
            },
      }
    };
}

pub struct SendToken;
impl CommandImplementation for SendToken {
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
        let signers_did = get_signers_did(args).unwrap();
        let signers_states = signers_did
            .iter()
            .map(|did| signers.get_signer_state(did).unwrap().clone())
            .collect::<Vec<_>>();
        let mut signer_state = signers.pop_signer_state(signers_did.first().unwrap()).unwrap();

        let amount = args
            .get_expected_uint(AMOUNT)
            .map_err(|e| (signers.clone(), signer_state.clone(), e))?;

        let token_mint_address = Pubkey::from_str(
            args.get_expected_string(TOKEN)
                .map_err(|e| (signers.clone(), signer_state.clone(), e))?,
        )
        .map_err(|e| {
            (
                signers.clone(),
                signer_state.clone(),
                diagnosed_error!("invalid token pubkey: {}", e.to_string()),
            )
        })?;

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

        let mut signer_pubkeys = vec![];
        for signer_state in signers_states.iter() {
            let signer_pubkey = signer_state
                .get_expected_string(CHECKED_PUBLIC_KEY)
                .map_err(|e| (signers.clone(), signer_state.clone(), diagnosed_error!("{e}")))?;
            let signer_pubkey = Pubkey::from_str(signer_pubkey).map_err(|e| {
                (
                    signers.clone(),
                    signer_state.clone(),
                    diagnosed_error!("invalid signer pubkey: {}", e.to_string()),
                )
            })?;
            signer_pubkeys.push(signer_pubkey);
        }

        // if the user has specified the authority pubkey, use it, otherwise use the first signer
        let authority_pubkey = if let Some(authority_pubkey) = args.get_string(AUTHORITY) {
            Pubkey::from_str(authority_pubkey).map_err(|e| {
                (
                    signers.clone(),
                    signer_state.clone(),
                    diagnosed_error!("invalid authority pubkey: {}", e.to_string()),
                )
            })?
        } else {
            signer_pubkeys[0].clone()
        };

        let source_token_address =
            spl_associated_token_account_interface::address::get_associated_token_address(
                &authority_pubkey,
                &token_mint_address,
            );
        let recipient_token_address =
            spl_associated_token_account_interface::address::get_associated_token_address(
                &recipient,
                &token_mint_address,
            );

        let mut instructions = VecDeque::from([spl_token_interface::instruction::transfer(
            &spl_token_interface::id(),
            &source_token_address,
            &recipient_token_address,
            &authority_pubkey,
            &signer_pubkeys.iter().map(|s| s).collect::<Vec<_>>(),
            amount,
        )
        .map_err(|e| {
            (
                signers.clone(),
                signer_state.clone(),
                diagnosed_error!("failed to create token transfer instruction: {}", e.to_string()),
            )
        })?]);

        let client = RpcClient::new(rpc_api_url);

        let do_create_account = match client.get_account(&recipient_token_address) {
            Ok(recipient_account) => recipient_account.lamports == 0,
            Err(e) => {
                if e.to_string().contains("AccountNotFound") {
                    true
                } else {
                    return Err((
                        signers.clone(),
                        signer_state.clone(),
                        diagnosed_error!(
                            "failed to get token recipient account: {}",
                            e.to_string()
                        ),
                    ));
                }
            }
        };

        let mut is_funding_recipient = false;
        if do_create_account {
            if args.get_bool(FUND_RECIPIENT).unwrap_or(false) {
                is_funding_recipient = true;
                instructions.push_front(
                    spl_associated_token_account_interface::instruction::create_associated_token_account(
                        &authority_pubkey,
                        &recipient,
                        &token_mint_address,
                        &spl_token_interface::id(),
                    ),
                );
            } else {
                return Err(
                    (
                        signers.clone(),
                        signer_state.clone(),
                        diagnosed_error!("cannot transfer token because recipient is unfunded; fund the recipient account or use the `fund_recipient = true` option")
                    )
                );
            }
        }

        let mut message =
            Message::new(&instructions.into_iter().collect::<Vec<_>>(), Some(&authority_pubkey));

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

        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            RECIPIENT_TOKEN_ADDRESS,
            SvmValue::pubkey(recipient_token_address.to_bytes().to_vec()),
        );
        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            RECIPIENT_ADDRESS,
            SvmValue::pubkey(recipient.to_bytes().to_vec()),
        );
        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            SOURCE_TOKEN_ADDRESS,
            SvmValue::pubkey(source_token_address.to_bytes().to_vec()),
        );
        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            AUTHORITY_ADDRESS,
            SvmValue::pubkey(authority_pubkey.to_bytes().to_vec()),
        );
        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            TOKEN_MINT_ADDRESS,
            SvmValue::pubkey(token_mint_address.to_bytes().to_vec()),
        );
        signer_state.insert_scoped_value(
            &construct_did.to_string(),
            IS_FUNDING_RECIPIENT,
            Value::bool(is_funding_recipient),
        );

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
            let (signers, signer_state, mut res_signing) = match run_signing_future {
                Ok(future) => match future.await {
                    Ok(res) => res,
                    Err(err) => return Err(err),
                },
                Err(err) => return Err(err),
            };

            let recipient_token_address = signer_state
                .get_scoped_value(&construct_did.to_string(), RECIPIENT_TOKEN_ADDRESS)
                .unwrap();
            let recipient_address = signer_state
                .get_scoped_value(&construct_did.to_string(), RECIPIENT_ADDRESS)
                .unwrap();
            let source_token_address = signer_state
                .get_scoped_value(&construct_did.to_string(), SOURCE_TOKEN_ADDRESS)
                .unwrap();
            let authority_address = signer_state
                .get_scoped_value(&construct_did.to_string(), AUTHORITY_ADDRESS)
                .unwrap();
            let token_mint_address = signer_state
                .get_scoped_value(&construct_did.to_string(), TOKEN_MINT_ADDRESS)
                .unwrap();
            let is_funding_recipient = signer_state
                .get_scoped_value(&construct_did.to_string(), IS_FUNDING_RECIPIENT)
                .unwrap();

            res_signing
                .outputs
                .insert(RECIPIENT_TOKEN_ADDRESS.into(), recipient_token_address.clone());
            res_signing.outputs.insert(RECIPIENT_ADDRESS.into(), recipient_address.clone());
            res_signing.outputs.insert(SOURCE_TOKEN_ADDRESS.into(), source_token_address.clone());
            res_signing.outputs.insert(AUTHORITY_ADDRESS.into(), authority_address.clone());
            res_signing.outputs.insert(TOKEN_MINT_ADDRESS.into(), token_mint_address.clone());
            res_signing.outputs.insert(IS_FUNDING_RECIPIENT.into(), is_funding_recipient.clone());

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
        let logger = LogDispatcher::new(construct_did.as_uuid(), "svm::send_token", &progress_tx);
        let recipient_token_address =
            SvmValue::to_pubkey(outputs.get_expected_value(RECIPIENT_TOKEN_ADDRESS).unwrap())
                .unwrap();
        let recipient_address =
            SvmValue::to_pubkey(outputs.get_expected_value(RECIPIENT_ADDRESS).unwrap()).unwrap();
        let source_token_address =
            SvmValue::to_pubkey(outputs.get_expected_value(SOURCE_TOKEN_ADDRESS).unwrap()).unwrap();
        let authority_address =
            SvmValue::to_pubkey(outputs.get_expected_value(AUTHORITY_ADDRESS).unwrap()).unwrap();
        let token_mint_address =
            SvmValue::to_pubkey(outputs.get_expected_value(TOKEN_MINT_ADDRESS).unwrap()).unwrap();
        let is_funding_recipient = outputs.get_bool(IS_FUNDING_RECIPIENT).unwrap_or(false);

        logger.info("Token Transfer", format!("Transferring token {}", token_mint_address));
        logger.info(
            "Token Transfer",
            format!(
                "Authority {} generated source token account {}",
                authority_address, source_token_address
            ),
        );
        logger.info(
            "Token Transfer",
            format!(
                "Recipient {} generated recipient token account {}",
                recipient_address, recipient_token_address
            ),
        );
        if is_funding_recipient {
            logger.info(
                "Token Transfer",
                format!(
                    "Authority {} will fund recipient token account {}",
                    authority_address, recipient_token_address
                ),
            );
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
