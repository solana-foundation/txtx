use std::collections::{HashMap, VecDeque};
use std::str::FromStr;

use solana_client::rpc_client::RpcClient;
use solana_instruction::Instruction;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_transaction::Transaction;
use spl_token_2022_interface::{extension::StateWithExtensions, state::Mint};
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
    MINT, MINT_ADDRESS, RECIPIENT, RECIPIENT_ADDRESS, RECIPIENT_ATA, RPC_API_URL, SENDER_ADDRESS,
    SENDER_ATA, TOKEN, TRANSACTION_BYTES,
};
use crate::typing::{SvmValue, SVM_PUBKEY};

use super::get_signers_did;
use super::setup_surfnet::tokens::get_token_by_name;
use super::sign_transaction::{check_signed_executability, run_signed_execution};

fn derive_send_token_associated_accounts(
    authority_pubkey: &Pubkey,
    mint_address: &Pubkey,
    recipient: &Pubkey,
    token_program_id: &Pubkey,
) -> (Pubkey, Pubkey) {
    let sender_ata =
        spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
            authority_pubkey,
            mint_address,
            token_program_id,
        );
    let recipient_ata =
        spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
            recipient,
            mint_address,
            token_program_id,
        );
    (sender_ata, recipient_ata)
}

fn create_token_transfer_instruction(
    token_program_id: &Pubkey,
    mint_pubkey: &Pubkey,
    sender_ata: &Pubkey,
    recipient_ata: &Pubkey,
    authority_pubkey: &Pubkey,
    signer_pubkeys: &[&Pubkey],
    amount: u64,
    decimals: u8,
) -> Result<Instruction, Diagnostic> {
    spl_token_2022_interface::instruction::transfer_checked(
        token_program_id,
        sender_ata,
        mint_pubkey,
        recipient_ata,
        authority_pubkey,
        signer_pubkeys,
        amount,
        decimals,
    )
    .map_err(|e| diagnosed_error!("failed to create token transfer instruction: {}", e))
}

fn unpack_mint_decimals(mint_data: &[u8]) -> Result<u8, Diagnostic> {
    Ok(StateWithExtensions::<Mint>::unpack(mint_data)
        .map_err(|e| {
            diagnosed_error!(
                "failed to unpack mint data: {}. Ensure that the provided mint address is valid and corresponds to a token mint account.",
                e
            )
        })?
        .base
        .decimals)
}

fn build_send_token_instructions(
    authority_pubkey: &Pubkey,
    mint_address: &Pubkey,
    recipient: &Pubkey,
    token_program_id: &Pubkey,
    sender_ata: &Pubkey,
    recipient_ata: &Pubkey,
    signer_pubkeys: &[Pubkey],
    amount: u64,
    decimals: u8,
    recipient_needs_funding: bool,
    fund_recipient: bool,
) -> Result<(VecDeque<Instruction>, bool), Diagnostic> {
    let signer_pubkey_refs = signer_pubkeys.iter().collect::<Vec<_>>();
    let mut instructions = VecDeque::from([create_token_transfer_instruction(
        token_program_id,
        mint_address,
        sender_ata,
        recipient_ata,
        authority_pubkey,
        &signer_pubkey_refs,
        amount,
        decimals,
    )?]);

    if !recipient_needs_funding {
        return Ok((instructions, false));
    }

    if !fund_recipient {
        return Err(diagnosed_error!("cannot transfer token because recipient is unfunded; fund the recipient account or use the `fund_recipient = true` option"));
    }

    instructions.push_front(
        spl_associated_token_account_interface::instruction::create_associated_token_account(
            authority_pubkey,
            recipient,
            mint_address,
            token_program_id,
        ),
    );
    Ok((instructions, true))
}

fn insert_send_token_outputs(
    signer_state: &mut ValueStore,
    construct_did: &ConstructDid,
    recipient_ata: &Pubkey,
    recipient: &Pubkey,
    sender_ata: &Pubkey,
    authority_pubkey: &Pubkey,
    mint_address: &Pubkey,
    is_funding_recipient: bool,
) {
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        RECIPIENT_ATA,
        SvmValue::pubkey(recipient_ata.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        RECIPIENT_ADDRESS,
        SvmValue::pubkey(recipient.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        SENDER_ATA,
        SvmValue::pubkey(sender_ata.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        AUTHORITY_ADDRESS,
        SvmValue::pubkey(authority_pubkey.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        SENDER_ADDRESS,
        SvmValue::pubkey(authority_pubkey.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        MINT_ADDRESS,
        SvmValue::pubkey(mint_address.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        IS_FUNDING_RECIPIENT,
        Value::bool(is_funding_recipient),
    );
}

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
                mint: {
                    documentation: "The program address for the token being sent. This is also known as the 'token mint account'. You may also provide the symbol of known mints such as 'usdc', 'wsol', or 'usdt'.",
                    typing: Type::string(),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                token: {
                    documentation: "Alias for `mint`. Prefer `mint` for new runbooks.",
                    typing: Type::string(),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                recipient: {
                    documentation: "The SVM address of the recipient. The associated token account will be derived from this address and the token address.",
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
                    documentation: "If set to `true` and the recipient token account does not exist, the action will create the recipient associated token account using lamports from the authority (or the first signer). The default is `false`.",
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
                },
                rpc_api_auth_token: {
                    documentation: "The HTTP authentication token to include in the headers when making API requests.",
                    typing: Type::string(),
                    optional: true,
                    tainting: false,
                    internal: false,
                    sensitive: true
                }
            ],
            outputs: [
                signature: {
                    documentation: "The transaction computed signature.",
                    typing: Type::string()
                },
                recipient_associated_token_address: {
                    documentation: "The recipient derived associated token account address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                sender_associated_token_address: {
                    documentation: "The sender derived associated token account address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                mint_address: {
                    documentation: "The token mint address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                authority_address: {
                    documentation: "The authority account address. If it was not provided as an input, this will be the same as the sender address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                sender_address: {
                    documentation: "The sender account address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                recipient_address: {
                    documentation: "The recipient account address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                is_funding_recipient: {
                    documentation: "Whether the transaction included a step to fund the recipient associated token account.",
                    typing: Type::bool()
                }
            ],
            example: txtx_addon_kit::indoc! {
                r#"action "send_usdc" "svm::send_token" {
                    description = "Send 5 USDC"
                    amount = 5000000
                    signers = [signer.caller]
                    recipient = "zbBjhHwuqyKMmz8ber5oUtJJ3ZV4B6ePmANfGyKzVGV"
                    mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
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

        let mint_address_str =
            args.get_string(MINT).or_else(|| args.get_string(TOKEN)).ok_or_else(|| {
                (
                    signers.clone(),
                    signer_state.clone(),
                    diagnosed_error!("missing required 'mint' input (or 'token' alias)"),
                )
            })?;

        // We assume mainnet for token symbol resolution, as it's the only network where we know the token symbols and addresses.
        let mint_address = match get_token_by_name("mainnet", mint_address_str) {
            Some(addr) => addr,
            _ => Pubkey::from_str(mint_address_str).map_err(|e| {
                (
                    signers.clone(),
                    signer_state.clone(),
                    diagnosed_error!("invalid mint pubkey: {}", e.to_string()),
                )
            })?,
        };

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

        let client = RpcClient::new(rpc_api_url);

        let (token_program_id, mint_data) = match client.get_account(&mint_address) {
            Ok(e) => (e.owner, e.data),
            Err(e) => Err((
                signers.clone(),
                signer_state.clone(),
                diagnosed_error!("failed to get token account: {}", e.to_string()),
            ))?,
        };

        let (sender_ata, recipient_ata) = derive_send_token_associated_accounts(
            &authority_pubkey,
            &mint_address,
            &recipient,
            &token_program_id,
        );

        let recipient_needs_funding = match client.get_account(&recipient_ata) {
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

        let mint_decimals = unpack_mint_decimals(&mint_data)
            .map_err(|e| (signers.clone(), signer_state.clone(), e))?;

        let (instructions, is_funding_recipient) = build_send_token_instructions(
            &authority_pubkey,
            &mint_address,
            &recipient,
            &token_program_id,
            &sender_ata,
            &recipient_ata,
            &signer_pubkeys,
            amount,
            mint_decimals,
            recipient_needs_funding,
            args.get_bool(FUND_RECIPIENT).unwrap_or(false),
        )
        .map_err(|e| (signers.clone(), signer_state.clone(), e))?;

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

        insert_send_token_outputs(
            &mut signer_state,
            construct_did,
            &recipient_ata,
            &recipient,
            &sender_ata,
            &authority_pubkey,
            &mint_address,
            is_funding_recipient,
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

            let recipient_address = signer_state
                .get_scoped_value(&construct_did.to_string(), RECIPIENT_ADDRESS)
                .unwrap();

            let authority_address = signer_state
                .get_scoped_value(&construct_did.to_string(), AUTHORITY_ADDRESS)
                .unwrap();
            let sender_address =
                signer_state.get_scoped_value(&construct_did.to_string(), SENDER_ADDRESS).unwrap();
            let token_mint_address =
                signer_state.get_scoped_value(&construct_did.to_string(), MINT_ADDRESS).unwrap();
            let is_funding_recipient = signer_state
                .get_scoped_value(&construct_did.to_string(), IS_FUNDING_RECIPIENT)
                .unwrap();
            let recipient_ata =
                signer_state.get_scoped_value(&construct_did.to_string(), RECIPIENT_ATA).unwrap();
            let sender_ata =
                signer_state.get_scoped_value(&construct_did.to_string(), SENDER_ATA).unwrap();

            res_signing.outputs.insert(RECIPIENT_ADDRESS.into(), recipient_address.clone());
            res_signing.outputs.insert(AUTHORITY_ADDRESS.into(), authority_address.clone());
            res_signing.outputs.insert(SENDER_ADDRESS.into(), sender_address.clone());
            res_signing.outputs.insert(MINT_ADDRESS.into(), token_mint_address.clone());
            res_signing.outputs.insert(IS_FUNDING_RECIPIENT.into(), is_funding_recipient.clone());
            res_signing.outputs.insert(RECIPIENT_ATA.into(), recipient_ata.clone());
            res_signing.outputs.insert(SENDER_ATA.into(), sender_ata.clone());

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
        let recipient_ata =
            SvmValue::to_pubkey(outputs.get_expected_value(RECIPIENT_ATA).unwrap()).unwrap();
        let recipient_address =
            SvmValue::to_pubkey(outputs.get_expected_value(RECIPIENT_ADDRESS).unwrap()).unwrap();
        let sender_ata =
            SvmValue::to_pubkey(outputs.get_expected_value(SENDER_ATA).unwrap()).unwrap();
        let sender_address =
            SvmValue::to_pubkey(outputs.get_expected_value(SENDER_ADDRESS).unwrap()).unwrap();
        let mint_address =
            SvmValue::to_pubkey(outputs.get_expected_value(MINT_ADDRESS).unwrap()).unwrap();
        let is_funding_recipient = outputs.get_bool(IS_FUNDING_RECIPIENT).unwrap_or(false);

        logger.info("Token Transfer", format!("Transferring token {}", mint_address));
        logger.info(
            "Token Transfer",
            format!(
                "Authority {} generated sender associated token account {}",
                sender_address, sender_ata
            ),
        );
        logger.info(
            "Token Transfer",
            format!(
                "Recipient {} generated recipient associated token account {}",
                recipient_address, recipient_ata
            ),
        );
        if is_funding_recipient {
            logger.info(
                "Token Transfer",
                format!(
                    "Authority {} will fund recipient associated token account {}",
                    sender_address, recipient_ata
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::SIGNERS;
    use spl_token_interface::instruction::TokenInstruction;
    use txtx_addon_kit::types::commands::PreCommandSpecification;
    use txtx_addon_kit::types::Did;

    fn new_pubkey(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn unpack_token_instruction(instruction: &Instruction) -> TokenInstruction<'_> {
        TokenInstruction::unpack(&instruction.data).expect("valid token instruction")
    }

    fn mint_data_with_decimals(decimals: u8) -> Vec<u8> {
        let mut mint_data = vec![0; 82];
        mint_data[44] = decimals;
        mint_data[45] = 1;
        mint_data
    }

    fn send_token_input_names() -> Vec<String> {
        match &*SEND_TOKEN {
            PreCommandSpecification::Atomic(spec) => {
                spec.inputs.iter().map(|i| i.name.clone()).collect()
            }
            PreCommandSpecification::Composite(_) => panic!("send_token should be atomic"),
        }
    }

    fn send_token_input_optional(input_name: &str) -> bool {
        match &*SEND_TOKEN {
            PreCommandSpecification::Atomic(spec) => {
                spec.inputs
                    .iter()
                    .find(|input| input.name == input_name)
                    .expect("input should exist")
                    .optional
            }
            PreCommandSpecification::Composite(_) => panic!("send_token should be atomic"),
        }
    }

    fn send_token_output_names() -> Vec<String> {
        match &*SEND_TOKEN {
            PreCommandSpecification::Atomic(spec) => {
                spec.outputs.iter().map(|output| output.name.clone()).collect()
            }
            PreCommandSpecification::Composite(_) => panic!("send_token should be atomic"),
        }
    }

    #[test]
    fn exposes_requested_command_inputs() {
        let input_names = send_token_input_names();

        assert!(input_names.contains(&AMOUNT.to_string()));
        assert!(input_names.contains(&MINT.to_string()));
        assert!(input_names.contains(&TOKEN.to_string()));
        assert!(input_names.contains(&RECIPIENT.to_string()));
        assert!(input_names.contains(&AUTHORITY.to_string()));
        assert!(input_names.contains(&FUND_RECIPIENT.to_string()));
        assert!(input_names.contains(&SIGNERS.to_string()));
        assert!(input_names.contains(&RPC_API_URL.to_string()));
        assert!(!send_token_input_optional(AMOUNT));
        assert!(send_token_input_optional(MINT));
        assert!(send_token_input_optional(TOKEN));
        assert!(!send_token_input_optional(RECIPIENT));
        assert!(!send_token_input_optional(SIGNERS));
        assert!(send_token_input_optional(AUTHORITY));
        assert!(send_token_input_optional(FUND_RECIPIENT));
    }

    #[test]
    fn exposes_requested_command_outputs() {
        let output_names = send_token_output_names();

        assert!(output_names.contains(&RECIPIENT_ADDRESS.to_string()));
        assert!(output_names.contains(&RECIPIENT_ATA.to_string()));
        assert!(output_names.contains(&SENDER_ADDRESS.to_string()));
        assert!(output_names.contains(&SENDER_ATA.to_string()));
        assert!(output_names.contains(&MINT_ADDRESS.to_string()));
    }

    #[test]
    fn derives_token_program_specific_associated_accounts() {
        let authority = new_pubkey(1);
        let mint = new_pubkey(2);
        let recipient = new_pubkey(3);

        let (sender_ata, recipient_ata) = derive_send_token_associated_accounts(
            &authority,
            &mint,
            &recipient,
            &spl_token_2022_interface::id(),
        );

        assert_eq!(
            sender_ata,
            spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
                &authority,
                &mint,
                &spl_token_2022_interface::id(),
            )
        );
        assert_eq!(
            recipient_ata,
            spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
                &recipient,
                &mint,
                &spl_token_2022_interface::id(),
            )
        );
    }

    #[test]
    fn unpacks_mint_decimals_from_mint_data() {
        assert_eq!(unpack_mint_decimals(&mint_data_with_decimals(6)).unwrap(), 6);
    }

    #[test]
    fn rejects_invalid_mint_data_when_unpacking_decimals() {
        let err = unpack_mint_decimals(&[0; 44]).unwrap_err();

        assert!(err.message.contains("failed to unpack mint data"));
    }

    #[test]
    fn builds_transfer_instruction_without_recipient_funding() {
        let authority = new_pubkey(1);
        let mint = new_pubkey(2);
        let recipient = new_pubkey(3);
        let signer = new_pubkey(4);
        let amount = 5_000;
        let decimals = 18;
        let (sender_ata, recipient_ata) = derive_send_token_associated_accounts(
            &authority,
            &mint,
            &recipient,
            &spl_token_interface::id(),
        );

        let (instructions, is_funding_recipient) = build_send_token_instructions(
            &authority,
            &mint,
            &recipient,
            &spl_token_interface::id(),
            &sender_ata,
            &recipient_ata,
            &[signer],
            amount,
            decimals,
            false,
            false,
        )
        .unwrap();

        assert!(!is_funding_recipient);
        assert_eq!(instructions.len(), 1);
        let transfer = &instructions[0];
        assert_eq!(transfer.program_id, spl_token_interface::id());
        assert_eq!(transfer.accounts[0].pubkey, sender_ata);
        assert!(transfer.accounts[0].is_writable);
        assert_eq!(transfer.accounts[1].pubkey, mint);
        assert!(!transfer.accounts[1].is_writable);
        assert_eq!(transfer.accounts[2].pubkey, recipient_ata);
        assert!(transfer.accounts[2].is_writable);
        assert_eq!(transfer.accounts[3].pubkey, authority);
        assert!(!transfer.accounts[3].is_writable);
        assert!(!transfer.accounts[3].is_signer);
        assert_eq!(transfer.accounts[4].pubkey, signer);
        assert!(!transfer.accounts[4].is_writable);
        assert!(transfer.accounts[4].is_signer);

        match unpack_token_instruction(transfer) {
            TokenInstruction::TransferChecked { amount: actual, decimals: actual_decimals } => {
                assert_eq!(actual, amount);
                assert_eq!(actual_decimals, decimals);
            }
            instruction => panic!("expected transfer_checked instruction, got {instruction:?}"),
        }
    }

    #[test]
    fn prepends_recipient_funding_instruction_when_requested() {
        let authority = new_pubkey(1);
        let mint = new_pubkey(2);
        let recipient = new_pubkey(3);
        let (sender_ata, recipient_ata) = derive_send_token_associated_accounts(
            &authority,
            &mint,
            &recipient,
            &spl_token_2022_interface::id(),
        );

        let (instructions, is_funding_recipient) = build_send_token_instructions(
            &authority,
            &mint,
            &recipient,
            &spl_token_2022_interface::id(),
            &sender_ata,
            &recipient_ata,
            &[],
            1,
            18,
            true,
            true,
        )
        .unwrap();

        assert!(is_funding_recipient);
        assert_eq!(instructions.len(), 2);
        assert_eq!(
            instructions[0].program_id,
            spl_associated_token_account_interface::program::id()
        );
        assert_eq!(instructions[0].accounts[0].pubkey, authority);
        assert!(instructions[0].accounts[0].is_signer);
        assert!(instructions[0].accounts[0].is_writable);
        assert_eq!(instructions[0].accounts[1].pubkey, recipient_ata);
        assert!(instructions[0].accounts[1].is_writable);
        assert_eq!(instructions[0].accounts[2].pubkey, recipient);
        assert_eq!(instructions[0].accounts[3].pubkey, mint);
        assert_eq!(instructions[0].accounts[5].pubkey, spl_token_2022_interface::id());
        assert_eq!(instructions[1].program_id, spl_token_2022_interface::id());
        assert_eq!(instructions[1].accounts[0].pubkey, sender_ata);
        assert_eq!(instructions[1].accounts[1].pubkey, mint);
        assert_eq!(instructions[1].accounts[2].pubkey, recipient_ata);
        assert_eq!(instructions[1].accounts[3].pubkey, authority);
    }

    #[test]
    fn rejects_unfunded_recipient_without_funding_option() {
        let authority = new_pubkey(1);
        let mint = new_pubkey(2);
        let recipient = new_pubkey(3);
        let (sender_ata, recipient_ata) = derive_send_token_associated_accounts(
            &authority,
            &mint,
            &recipient,
            &spl_token_interface::id(),
        );

        let err = build_send_token_instructions(
            &authority,
            &mint,
            &recipient,
            &spl_token_interface::id(),
            &sender_ata,
            &recipient_ata,
            &[],
            1,
            18,
            true,
            false,
        )
        .unwrap_err();

        assert!(err.message.contains("recipient is unfunded"));
    }

    #[test]
    fn stores_send_token_outputs_in_signer_state() {
        let construct_did = ConstructDid(Did::from_components(vec![b"send_token"]));
        let signer_did = Did::from_components(vec![b"signer"]);
        let mut signer_state = ValueStore::new("authority", &signer_did);
        let authority = new_pubkey(1);
        let mint = new_pubkey(2);
        let recipient = new_pubkey(3);
        let (sender_ata, recipient_ata) = derive_send_token_associated_accounts(
            &authority,
            &mint,
            &recipient,
            &spl_token_interface::id(),
        );

        insert_send_token_outputs(
            &mut signer_state,
            &construct_did,
            &recipient_ata,
            &recipient,
            &sender_ata,
            &authority,
            &mint,
            true,
        );

        let scope = construct_did.to_string();
        assert_eq!(
            SvmValue::to_pubkey(signer_state.get_scoped_value(&scope, RECIPIENT_ATA).unwrap())
                .unwrap(),
            recipient_ata
        );
        assert_eq!(
            SvmValue::to_pubkey(signer_state.get_scoped_value(&scope, RECIPIENT_ADDRESS).unwrap())
                .unwrap(),
            recipient
        );
        assert_eq!(
            SvmValue::to_pubkey(signer_state.get_scoped_value(&scope, SENDER_ATA).unwrap())
                .unwrap(),
            sender_ata
        );
        assert_eq!(
            SvmValue::to_pubkey(signer_state.get_scoped_value(&scope, AUTHORITY_ADDRESS).unwrap())
                .unwrap(),
            authority
        );
        assert_eq!(
            SvmValue::to_pubkey(signer_state.get_scoped_value(&scope, SENDER_ADDRESS).unwrap())
                .unwrap(),
            authority
        );
        assert_eq!(
            SvmValue::to_pubkey(signer_state.get_scoped_value(&scope, MINT_ADDRESS).unwrap())
                .unwrap(),
            mint
        );
        assert!(signer_state.get_scoped_value(&scope, IS_FUNDING_RECIPIENT).unwrap().expect_bool());
    }
}
