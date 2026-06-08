use std::collections::HashMap;
use std::str::FromStr;

use solana_client::rpc_client::RpcClient;
use solana_instruction::Instruction;
use solana_keypair::{Keypair, Signer};
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
use txtx_addon_kit::types::types::{RunbookSupervisionContext, Type};
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;
use txtx_addon_network_svm_types::SVM_PUBKEY;

use crate::codec::send_transaction::send_transaction_background_task;
use crate::constants::{
    AUTHORITY, AUTHORITY_ADDRESS, CHECKED_PUBLIC_KEY, DECIMALS, FREEZE_AUTHORITY, INITIAL_SUPPLY,
    PAYER, RPC_API_URL, SIGNERS, TOKEN_MINT_ADDRESS, TOKEN_PROGRAM_ID, TRANSACTION_BYTES,
};
use crate::typing::SvmValue;

use super::get_signers_did;
use super::sign_transaction::{check_signed_executability, run_signed_execution};

// Base SPL Token mint size. Token-2022 mints without extensions use the same size,
// but Token-2022 extensions require allocating additional account space and
// initializing extension-specific state before initialize_mint. This command
// currently creates basic mints only.
const TOKEN_MINT_ACCOUNT_SPACE: u64 = 82;

fn create_initialize_mint_instruction(
    token_program_id: &Pubkey,
    mint_pubkey: &Pubkey,
    authority_pubkey: &Pubkey,
    freeze_authority_pubkey: Option<&Pubkey>,
    decimals: u8,
) -> Result<Instruction, Diagnostic> {
    if token_program_id == &spl_token_interface::id() {
        return spl_token_interface::instruction::initialize_mint(
            token_program_id,
            mint_pubkey,
            authority_pubkey,
            freeze_authority_pubkey,
            decimals,
        )
        .map_err(|e| {
            diagnosed_error!("failed to create token mint initialization instruction: {}", e)
        });
    }

    if token_program_id == &spl_token_2022_interface::id() {
        return spl_token_2022_interface::instruction::initialize_mint(
            token_program_id,
            mint_pubkey,
            authority_pubkey,
            freeze_authority_pubkey,
            decimals,
        )
        .map_err(|e| {
            diagnosed_error!("failed to create token mint initialization instruction: {}", e)
        });
    }

    Err(diagnosed_error!(
        "unsupported token program id: {}; expected SPL Token ({}) or Token-2022 ({})",
        token_program_id,
        spl_token_interface::id(),
        spl_token_2022_interface::id()
    ))
}

fn create_mint_to_instruction(
    token_program_id: &Pubkey,
    mint_pubkey: &Pubkey,
    authority_token_account: &Pubkey,
    authority_pubkey: &Pubkey,
    signer_pubkeys: &[&Pubkey],
    initial_supply: u64,
) -> Result<Instruction, Diagnostic> {
    if token_program_id == &spl_token_interface::id() {
        return spl_token_interface::instruction::mint_to(
            token_program_id,
            mint_pubkey,
            authority_token_account,
            authority_pubkey,
            signer_pubkeys,
            initial_supply,
        )
        .map_err(|e| diagnosed_error!("failed to create token mint-to instruction: {}", e));
    }

    if token_program_id == &spl_token_2022_interface::id() {
        return spl_token_2022_interface::instruction::mint_to(
            token_program_id,
            mint_pubkey,
            authority_token_account,
            authority_pubkey,
            signer_pubkeys,
            initial_supply,
        )
        .map_err(|e| diagnosed_error!("failed to create token mint-to instruction: {}", e));
    }

    Err(diagnosed_error!(
        "unsupported token program id: {}; expected SPL Token ({}) or Token-2022 ({})",
        token_program_id,
        spl_token_interface::id(),
        spl_token_2022_interface::id()
    ))
}

fn build_deploy_token_instructions(
    payer_pubkey: &Pubkey,
    mint_pubkey: &Pubkey,
    authority_pubkey: &Pubkey,
    freeze_authority_pubkey: Option<&Pubkey>,
    decimals: u8,
    initial_supply: u64,
    mint_lamports: u64,
    signer_pubkeys: &[Pubkey],
    token_program_id: &Pubkey,
) -> Result<Vec<Instruction>, Diagnostic> {
    let mut instructions = vec![
        solana_system_interface::instruction::create_account(
            payer_pubkey,
            mint_pubkey,
            mint_lamports,
            TOKEN_MINT_ACCOUNT_SPACE,
            token_program_id,
        ),
        create_initialize_mint_instruction(
            token_program_id,
            mint_pubkey,
            authority_pubkey,
            freeze_authority_pubkey,
            decimals,
        )?,
    ];

    if initial_supply > 0 {
        let authority_token_account =
            spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
                authority_pubkey,
                mint_pubkey,
                token_program_id,
            );
        instructions.push(
            spl_associated_token_account_interface::instruction::create_associated_token_account_idempotent(
                payer_pubkey,
                authority_pubkey,
                mint_pubkey,
                token_program_id,
            ),
        );

        let signer_pubkey_refs =
            if signer_pubkeys.len() == 1 && signer_pubkeys[0] == *authority_pubkey {
                Vec::new()
            } else {
                signer_pubkeys.iter().collect::<Vec<_>>()
            };
        instructions.push(create_mint_to_instruction(
            token_program_id,
            mint_pubkey,
            &authority_token_account,
            authority_pubkey,
            &signer_pubkey_refs,
            initial_supply,
        )?);
    }

    Ok(instructions)
}

fn insert_deploy_token_outputs(
    signer_state: &mut ValueStore,
    construct_did: &ConstructDid,
    mint_pubkey: &Pubkey,
    authority_pubkey: &Pubkey,
) {
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        TOKEN_MINT_ADDRESS,
        SvmValue::pubkey(mint_pubkey.to_bytes().to_vec()),
    );
    signer_state.insert_scoped_value(
        &construct_did.to_string(),
        AUTHORITY_ADDRESS,
        SvmValue::pubkey(authority_pubkey.to_bytes().to_vec()),
    );
}

lazy_static! {
    pub static ref DEPLOY_TOKEN: PreCommandSpecification = define_command! {
        DeployToken => {
            name: "Deploy SVM Token",
            matcher: "deploy_token",
            documentation: "The `svm::deploy_token` action creates an SPL token mint, signs the transaction, and broadcasts it to the network.",
            implements_signing_capability: true,
            implements_background_task_capability: true,
            inputs: [
                description: {
                    documentation: "A description of the token deployment action.",
                    typing: Type::string(),
                    optional: true,
                    tainting: false,
                    internal: false,
                    sensitive: false
                },
                decimals: {
                    documentation: "The number of decimal places for the token mint.",
                    typing: Type::integer(),
                    optional: false,
                    tainting: false,
                    internal: false,
                    sensitive: false
                },
                initial_supply: {
                    documentation: "The optional initial token supply to mint to the authority associated token account, in base units. Defaults to 0.",
                    typing: Type::integer(),
                    optional: true,
                    tainting: false,
                    internal: false,
                    sensitive: false
                },
                authority: {
                    documentation: "The pubkey of the mint authority. If omitted, the first signer will be used.",
                    typing: Type::string(),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                payer: {
                    documentation: "A reference to a signer construct, which will be used to pay for token mint account creation. If omitted, the first signer will be used.",
                    typing: Type::string(),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                freeze_authority: {
                    documentation: "The optional pubkey of the freeze authority for the token mint.",
                    typing: Type::string(),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                signers: {
                    documentation: "A set of references to signer constructs, which will be used to sign the token deployment transaction.",
                    typing: Type::array(Type::string()),
                    optional: false,
                    tainting: true,
                    internal: false,
                    sensitive: false
                },
                token_program_id: {
                    documentation: "The optional token program id to use for the token mint. If omitted, the standard SPL Token program id will be used. Supported values are the SPL Token and Token-2022 program ids. Token-2022 support creates basic mints only; extensions are not configured or pre-allocated by this command.",
                    typing: Type::addon(SVM_PUBKEY),
                    optional: true,
                    tainting: true,
                    internal: false,
                    sensitive: false
                }
            ],
            outputs: [
                signature: {
                    documentation: "The transaction computed signature.",
                    typing: Type::string()
                },
                token_mint_address: {
                    documentation: "The token mint address.",
                    typing: Type::addon(SVM_PUBKEY)
                },
                authority_address: {
                    documentation: "The mint authority address.",
                    typing: Type::addon(SVM_PUBKEY)
                }
            ],
            example: txtx_addon_kit::indoc! {
                r#"action "deploy_token" "svm::deploy_token" {
                    description = "Deploy an SPL token"
                    decimals = 6
                    initial_supply = 1000000
                    signers = [signer.authority]
                }"#
            },
      }
    };
}

pub struct DeployToken;
impl CommandImplementation for DeployToken {
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
        let signers_did = get_signers_did(args)
            .map_err(|e| (signers.clone(), ValueStore::tmp(), diagnosed_error!("{e}")))?;
        let first_signer_did = signers_did.first().cloned().ok_or_else(|| {
            (signers.clone(), ValueStore::tmp(), diagnosed_error!("signers list is empty"))
        })?;

        let signers_states = signers_did
            .iter()
            .map(|did| {
                signers.get_signer_state(did).cloned().ok_or_else(|| {
                    (
                        signers.clone(),
                        ValueStore::tmp(),
                        diagnosed_error!("signer state not found for signer {}", did),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let payer_signer_did =
            super::get_custom_signer_did(args, PAYER).unwrap_or_else(|_| first_signer_did.clone());
        let mut payer_signer_state =
            signers.pop_signer_state(&payer_signer_did).ok_or_else(|| {
                (
                    signers.clone(),
                    ValueStore::tmp(),
                    diagnosed_error!("signer state not found for payer {}", payer_signer_did),
                )
            })?;

        let decimals = args
            .get_expected_uint(DECIMALS)
            .map_err(|e| (signers.clone(), payer_signer_state.clone(), e))?;
        let decimals = u8::try_from(decimals).map_err(|_| {
            (
                signers.clone(),
                payer_signer_state.clone(),
                diagnosed_error!("invalid decimals: value must fit in u8"),
            )
        })?;

        let initial_supply = args
            .get_uint(INITIAL_SUPPLY)
            .map_err(|e| (signers.clone(), payer_signer_state.clone(), diagnosed_error!("{e}")))?
            .unwrap_or(0);

        let rpc_api_url = args
            .get_expected_string(RPC_API_URL)
            .map_err(|e| (signers.clone(), payer_signer_state.clone(), e))?
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
                    diagnosed_error!("invalid signer pubkey: {}", e),
                )
            })?;
            signer_pubkeys.push(signer_pubkey);
        }

        let payer_pubkey = payer_signer_state
            .get_expected_string(CHECKED_PUBLIC_KEY)
            .map_err(|e| (signers.clone(), payer_signer_state.clone(), diagnosed_error!("{e}")))?;
        let payer_pubkey = Pubkey::from_str(payer_pubkey).map_err(|e| {
            (
                signers.clone(),
                payer_signer_state.clone(),
                diagnosed_error!("invalid payer pubkey: {}", e),
            )
        })?;

        let authority_pubkey = if let Some(authority_pubkey) = args.get_string(AUTHORITY) {
            Pubkey::from_str(authority_pubkey).map_err(|e| {
                (
                    signers.clone(),
                    payer_signer_state.clone(),
                    diagnosed_error!("invalid authority pubkey: {}", e),
                )
            })?
        } else {
            *signer_pubkeys.first().ok_or_else(|| {
                (
                    signers.clone(),
                    payer_signer_state.clone(),
                    diagnosed_error!("signers list is empty"),
                )
            })?
        };

        let freeze_authority_pubkey = args
            .get_string(FREEZE_AUTHORITY)
            .map(|freeze_authority| {
                Pubkey::from_str(freeze_authority).map_err(|e| {
                    (
                        signers.clone(),
                        payer_signer_state.clone(),
                        diagnosed_error!("invalid freeze authority pubkey: {}", e),
                    )
                })
            })
            .transpose()?;

        let mint_keypair = Keypair::new();
        let mint_pubkey = mint_keypair.pubkey();

        let token_program_id = args
            .get_value(TOKEN_PROGRAM_ID)
            .map(|id| {
                SvmValue::to_pubkey(id).map_err(|e| {
                    (
                        signers.clone(),
                        payer_signer_state.clone(),
                        diagnosed_error!("invalid token program id: {}", e),
                    )
                })
            })
            .transpose()?
            .unwrap_or(spl_token_interface::id());

        let client = RpcClient::new(rpc_api_url);
        let mint_lamports = client
            .get_minimum_balance_for_rent_exemption(TOKEN_MINT_ACCOUNT_SPACE as usize)
            .map_err(|e| {
                (
                    signers.clone(),
                    payer_signer_state.clone(),
                    diagnosed_error!("failed to get token mint rent exemption: {}", e),
                )
            })?;

        let instructions = build_deploy_token_instructions(
            &payer_pubkey,
            &mint_pubkey,
            &authority_pubkey,
            freeze_authority_pubkey.as_ref(),
            decimals,
            initial_supply,
            mint_lamports,
            &signer_pubkeys,
            &token_program_id,
        )
        .map_err(|e| (signers.clone(), payer_signer_state.clone(), e))?;

        let mut message = Message::new(&instructions, Some(&payer_pubkey));
        message.recent_blockhash = client.get_latest_blockhash().map_err(|e| {
            (
                signers.clone(),
                payer_signer_state.clone(),
                diagnosed_error!("failed to retrieve latest blockhash: {}", e),
            )
        })?;

        let mut transaction = Transaction::new_unsigned(message);
        transaction
            .try_partial_sign(&[&mint_keypair], transaction.message.recent_blockhash)
            .map_err(|e| {
                (
                    signers.clone(),
                    payer_signer_state.clone(),
                    diagnosed_error!("failed to sign token mint account creation: {}", e),
                )
            })?;
        let transaction = SvmValue::transaction(&transaction)
            .map_err(|diag| (signers.clone(), payer_signer_state.clone(), diag))?;

        let mut args = args.clone();
        args.insert(TRANSACTION_BYTES, transaction);
        let mut effective_signers_did = vec![payer_signer_did.clone()];
        if initial_supply > 0 {
            for signer_did in signers_did.iter() {
                if !effective_signers_did.contains(signer_did) {
                    effective_signers_did.push(signer_did.clone());
                }
            }
        }
        args.insert(
            SIGNERS,
            txtx_addon_kit::types::types::Value::array(
                effective_signers_did
                    .iter()
                    .map(|d| txtx_addon_kit::types::types::Value::string(d.to_string()))
                    .collect(),
            ),
        );

        insert_deploy_token_outputs(
            &mut payer_signer_state,
            construct_did,
            &mint_pubkey,
            &authority_pubkey,
        );
        for signer_did in effective_signers_did.iter() {
            if signer_did == &payer_signer_did {
                continue;
            }
            let Some(signer_state) = signers.get_signer_state_mut(signer_did) else {
                return Err((
                    signers.clone(),
                    payer_signer_state.clone(),
                    diagnosed_error!("signer state not found for signer {}", signer_did),
                ));
            };
            insert_deploy_token_outputs(
                signer_state,
                construct_did,
                &mint_pubkey,
                &authority_pubkey,
            );
        }

        signers.push_signer_state(payer_signer_state);
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

            let token_mint_address = signer_state
                .get_scoped_value(&construct_did.to_string(), TOKEN_MINT_ADDRESS)
                .cloned()
                .ok_or_else(|| {
                    (
                        signers.clone(),
                        signer_state.clone(),
                        diagnosed_error!("missing token mint address output for deploy_token"),
                    )
                })?;
            let authority_address = signer_state
                .get_scoped_value(&construct_did.to_string(), AUTHORITY_ADDRESS)
                .cloned()
                .ok_or_else(|| {
                    (
                        signers.clone(),
                        signer_state.clone(),
                        diagnosed_error!("missing authority address output for deploy_token"),
                    )
                })?;

            res_signing.outputs.insert(TOKEN_MINT_ADDRESS.into(), token_mint_address);
            res_signing.outputs.insert(AUTHORITY_ADDRESS.into(), authority_address);

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
        let logger = LogDispatcher::new(construct_did.as_uuid(), "svm::deploy_token", &progress_tx);
        let token_mint_address = outputs
            .get_expected_value(TOKEN_MINT_ADDRESS)
            .map_err(|e| diagnosed_error!("missing token mint address output: {e}"))
            .and_then(|value| {
                SvmValue::to_pubkey(value)
                    .map_err(|e| diagnosed_error!("invalid token mint address output: {e}"))
            })?;
        let authority_address = outputs
            .get_expected_value(AUTHORITY_ADDRESS)
            .map_err(|e| diagnosed_error!("missing authority address output: {e}"))
            .and_then(|value| {
                SvmValue::to_pubkey(value)
                    .map_err(|e| diagnosed_error!("invalid authority address output: {e}"))
            })?;

        logger.info("Token Deployment", format!("Deploying token {}", token_mint_address));
        logger.info("Token Deployment", format!("Mint authority: {}", authority_address));

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
    use solana_system_interface::instruction::SystemInstruction;
    use spl_token_interface::instruction::TokenInstruction;
    use txtx_addon_kit::types::commands::PreCommandSpecification;
    use txtx_addon_kit::types::Did;

    fn new_pubkey(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn unpack_system_instruction(instruction: &Instruction) -> SystemInstruction {
        bincode::deserialize(&instruction.data).expect("valid system instruction")
    }

    fn unpack_token_instruction(instruction: &Instruction) -> TokenInstruction<'_> {
        TokenInstruction::unpack(&instruction.data).expect("valid token instruction")
    }

    fn deploy_token_input_names() -> Vec<String> {
        match &*DEPLOY_TOKEN {
            PreCommandSpecification::Atomic(spec) => {
                spec.inputs.iter().map(|i| i.name.clone()).collect()
            }
            PreCommandSpecification::Composite(_) => panic!("deploy_token should be atomic"),
        }
    }

    #[test]
    fn stores_deployment_outputs_in_signer_state() {
        let construct_did = ConstructDid(Did::from_components(vec![b"deploy_token"]));
        let signer_did = Did::from_components(vec![b"signer"]);
        let mut signer_state = ValueStore::new("authority", &signer_did);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);

        insert_deploy_token_outputs(&mut signer_state, &construct_did, &mint, &authority);

        let token_mint_address = signer_state
            .get_scoped_value(&construct_did.to_string(), TOKEN_MINT_ADDRESS)
            .expect("token mint address output should exist");
        let authority_address = signer_state
            .get_scoped_value(&construct_did.to_string(), AUTHORITY_ADDRESS)
            .expect("authority address output should exist");

        assert_eq!(SvmValue::to_pubkey(token_mint_address).unwrap(), mint);
        assert_eq!(SvmValue::to_pubkey(authority_address).unwrap(), authority);
    }

    #[test]
    fn exposes_requested_command_inputs() {
        let input_names = deploy_token_input_names();

        assert!(input_names.contains(&PAYER.to_string()));
        assert!(input_names.contains(&TOKEN_PROGRAM_ID.to_string()));
        assert!(!input_names.contains(&"mint_keypair".to_string()));
        assert!(!input_names.contains(&"mint".to_string()));
        assert!(!input_names.contains(&"rpc_api_url".to_string()));
        assert!(!input_names.contains(&"rpc_api_auth_token".to_string()));
    }

    #[test]
    fn builds_minimal_token_deployment_instructions() {
        let payer = new_pubkey(1);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);
        let mint_lamports = 1_234_567;

        let instructions = build_deploy_token_instructions(
            &payer,
            &mint,
            &authority,
            None,
            6,
            0,
            mint_lamports,
            &[authority],
            &spl_token_interface::id(),
        )
        .unwrap();

        assert_eq!(instructions.len(), 2);

        assert_eq!(instructions[0].program_id, solana_system_interface::program::id());
        assert_eq!(instructions[0].accounts.len(), 2);
        assert_eq!(instructions[0].accounts[0].pubkey, payer);
        assert!(instructions[0].accounts[0].is_signer);
        assert!(instructions[0].accounts[0].is_writable);
        assert_eq!(instructions[0].accounts[1].pubkey, mint);
        assert!(instructions[0].accounts[1].is_signer);
        assert!(instructions[0].accounts[1].is_writable);

        match unpack_system_instruction(&instructions[0]) {
            SystemInstruction::CreateAccount { lamports, space, owner } => {
                assert_eq!(lamports, mint_lamports);
                assert_eq!(space, TOKEN_MINT_ACCOUNT_SPACE);
                assert_eq!(owner, spl_token_interface::id());
            }
            instruction => panic!("expected create account instruction, got {instruction:?}"),
        }

        assert_eq!(instructions[1].program_id, spl_token_interface::id());
        assert_eq!(instructions[1].accounts[0].pubkey, mint);
        assert!(instructions[1].accounts[0].is_writable);

        match unpack_token_instruction(&instructions[1]) {
            TokenInstruction::InitializeMint { decimals, mint_authority, freeze_authority } => {
                assert_eq!(decimals, 6);
                assert_eq!(mint_authority, authority);
                assert!(freeze_authority.is_none());
            }
            instruction => panic!("expected initialize mint instruction, got {instruction:?}"),
        }
    }

    #[test]
    fn builds_initial_supply_instructions() {
        let payer = new_pubkey(1);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);
        let initial_supply = 1_000_000;

        let instructions = build_deploy_token_instructions(
            &payer,
            &mint,
            &authority,
            None,
            9,
            initial_supply,
            500,
            &[authority],
            &spl_token_interface::id(),
        )
        .unwrap();

        assert_eq!(instructions.len(), 4);

        let expected_ata =
            spl_associated_token_account_interface::address::get_associated_token_address(
                &authority, &mint,
            );

        assert_eq!(
            instructions[2].program_id,
            spl_associated_token_account_interface::program::id()
        );
        assert_eq!(instructions[2].accounts[0].pubkey, payer);
        assert_eq!(instructions[2].accounts[1].pubkey, expected_ata);
        assert_eq!(instructions[2].accounts[2].pubkey, authority);
        assert_eq!(instructions[2].accounts[3].pubkey, mint);

        assert_eq!(instructions[3].program_id, spl_token_interface::id());
        assert_eq!(instructions[3].accounts[0].pubkey, mint);
        assert!(instructions[3].accounts[0].is_writable);
        assert_eq!(instructions[3].accounts[1].pubkey, expected_ata);
        assert!(instructions[3].accounts[1].is_writable);
        assert_eq!(instructions[3].accounts[2].pubkey, authority);
        assert!(instructions[3].accounts[2].is_signer);
        assert_eq!(instructions[3].accounts.len(), 3);

        match unpack_token_instruction(&instructions[3]) {
            TokenInstruction::MintTo { amount } => assert_eq!(amount, initial_supply),
            instruction => panic!("expected mint_to instruction, got {instruction:?}"),
        }
    }

    #[test]
    fn builds_token_2022_deployment_instructions() {
        let payer = new_pubkey(1);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);
        let initial_supply = 1_000_000;

        let instructions = build_deploy_token_instructions(
            &payer,
            &mint,
            &authority,
            None,
            9,
            initial_supply,
            500,
            &[authority],
            &spl_token_2022_interface::id(),
        )
        .unwrap();

        let expected_ata =
            spl_associated_token_account_interface::address::get_associated_token_address_with_program_id(
                &authority,
                &mint,
                &spl_token_2022_interface::id(),
            );

        assert_eq!(instructions.len(), 4);
        match unpack_system_instruction(&instructions[0]) {
            SystemInstruction::CreateAccount { owner, .. } => {
                assert_eq!(owner, spl_token_2022_interface::id());
            }
            instruction => panic!("expected create account instruction, got {instruction:?}"),
        }
        assert_eq!(instructions[1].program_id, spl_token_2022_interface::id());
        assert_eq!(
            instructions[2].program_id,
            spl_associated_token_account_interface::program::id()
        );
        assert_eq!(instructions[2].accounts[1].pubkey, expected_ata);
        assert_eq!(instructions[3].program_id, spl_token_2022_interface::id());
        assert_eq!(instructions[3].accounts[1].pubkey, expected_ata);
        assert_eq!(instructions[3].accounts[2].pubkey, authority);
        assert!(instructions[3].accounts[2].is_signer);
        assert_eq!(instructions[3].accounts.len(), 3);
    }

    #[test]
    fn rejects_unsupported_token_program_id() {
        let payer = new_pubkey(1);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);
        let unsupported_program = new_pubkey(9);

        let err = build_deploy_token_instructions(
            &payer,
            &mint,
            &authority,
            None,
            6,
            0,
            500,
            &[payer],
            &unsupported_program,
        )
        .unwrap_err();

        assert!(err.message.contains("unsupported token program id"));
    }

    #[test]
    fn uses_freeze_authority_when_provided() {
        let payer = new_pubkey(1);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);
        let freeze_authority = new_pubkey(4);

        let instructions = build_deploy_token_instructions(
            &payer,
            &mint,
            &authority,
            Some(&freeze_authority),
            2,
            0,
            500,
            &[payer],
            &spl_token_interface::id(),
        )
        .unwrap();

        match unpack_token_instruction(&instructions[1]) {
            TokenInstruction::InitializeMint { freeze_authority: actual, .. } => {
                assert_eq!(actual, freeze_authority.into());
            }
            instruction => panic!("expected initialize mint instruction, got {instruction:?}"),
        }
    }

    #[test]
    fn preserves_multisig_signer_pubkeys_for_mint_to() {
        let payer = new_pubkey(1);
        let mint = new_pubkey(2);
        let authority = new_pubkey(3);
        let signer_one = new_pubkey(4);
        let signer_two = new_pubkey(5);

        let instructions = build_deploy_token_instructions(
            &payer,
            &mint,
            &authority,
            None,
            6,
            42,
            500,
            &[signer_one, signer_two],
            &spl_token_interface::id(),
        )
        .unwrap();

        assert_eq!(instructions[3].accounts[2].pubkey, authority);
        assert!(!instructions[3].accounts[2].is_signer);
        assert_eq!(instructions[3].accounts[3].pubkey, signer_one);
        assert!(instructions[3].accounts[3].is_signer);
        assert_eq!(instructions[3].accounts[4].pubkey, signer_two);
        assert!(instructions[3].accounts[4].is_signer);

        match unpack_token_instruction(&instructions[3]) {
            TokenInstruction::MintTo { amount } => assert_eq!(amount, 42),
            instruction => panic!("expected mint_to instruction, got {instruction:?}"),
        }
    }
}
