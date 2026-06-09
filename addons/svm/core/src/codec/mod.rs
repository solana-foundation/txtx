pub mod anchor;
pub mod idl;
pub mod instruction;
pub mod native;
pub mod send_transaction;
pub mod squads;
pub mod ui_encode;
pub mod utils;

use crate::codec::ui_encode::get_formatted_transaction_meta_description;
use crate::codec::ui_encode::message_to_formatted_tx;
use crate::codec::utils::wait_n_slots;
use crate::commands::RpcVersionInfo;
use anchor::AnchorProgramArtifacts;
use bip39::Language;
use bip39::Mnemonic;
use bip39::MnemonicType;
use bip39::Seed;
use native::NativeProgramArtifacts;
use serde::Deserialize;
use serde::Serialize;
use solana_client::rpc_client::RpcClient;
use solana_account::state_traits::StateMut;
use solana_loader_v3_interface::{get_program_data_address, instruction::create_buffer, state::UpgradeableLoaderState};
use solana_commitment_config::CommitmentConfig;
use solana_commitment_config::CommitmentLevel;
use solana_hash::Hash;
use solana_packet::PACKET_DATA_SIZE;
use solana_keypair::{Keypair, keypair_from_seed};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
use solana_system_interface::MAX_PERMITTED_DATA_LENGTH;
use txtx_addon_kit::types::frontend::LogDispatcher;
use crate::typing::DeploymentTransactionType;
use solana_loader_v3_interface::instruction as bpf_loader_upgradeable;
use solana_instruction::Instruction;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_transaction::Transaction;
use std::collections::HashMap;
use std::str::FromStr;
use txtx_addon_kit::types::diagnostics::Diagnostic;
use txtx_addon_kit::types::signers::SignerInstance;
use txtx_addon_kit::types::types::Value;
use txtx_addon_kit::types::ConstructDid;

use crate::constants::AUTHORITY;
use crate::constants::PAYER;
use crate::typing::SvmValue;
use crate::typing::SVM_CLOSE_TEMP_AUTHORITY_TRANSACTION_PARTS;
use crate::typing::SVM_DEPLOYMENT_TRANSACTION;

const LAMPORTS_PER_SIGNATURE: u64 = 5000;

pub fn public_key_from_str(str: &str) -> Result<Pubkey, Diagnostic> {
    Pubkey::from_str(str).map_err(|e| diagnosed_error!("invalid public key: {e}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TxtxDeploymentSigner {
    Payer,
    FinalAuthority,
}
impl TxtxDeploymentSigner {
    pub fn to_signer_key(&self) -> String {
        match self {
            Self::Payer => PAYER.to_string(),
            Self::FinalAuthority => AUTHORITY.to_string(),
        }
    }
}

/// `transaction_with_keypairs` - The transaction to sign, with the keypairs we have on hand to sign them
/// `signers` - The txtx signers (if any) that need to sign the transaction
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentTransaction {
    pub signers: Option<Vec<TxtxDeploymentSigner>>,
    pub transaction: Option<Transaction>,
    pub keypairs_bytes: Vec<Vec<u8>>,
    pub transaction_type: DeploymentTransactionType,
    pub commitment_level: CommitmentLevel,
    pub do_await_confirmation: bool,
    pub cheatcode_data: Option<(Pubkey, Vec<u8>)>,
}

impl DeploymentTransaction {
    pub fn new(
        transaction: &Transaction,
        keypairs: Vec<&Keypair>,
        signers: Option<Vec<TxtxDeploymentSigner>>,
        transaction_type: DeploymentTransactionType,
        commitment_level: CommitmentLevel,
        do_await_confirmation: bool,
    ) -> Self {
        let keypairs_bytes = keypairs.iter().map(|k| k.to_bytes().to_vec()).collect();
        Self {
            signers,
            transaction: Some(transaction.clone()),
            keypairs_bytes,
            transaction_type,
            commitment_level,
            do_await_confirmation,
            cheatcode_data: None,
        }
    }

    pub fn new_cheatcode_deployment(
        transaction_type: DeploymentTransactionType,
        cheatcode_data: (Pubkey, Vec<u8>),
    ) -> Self {
        Self {
            signers: None,
            transaction: None,
            keypairs_bytes: Vec::new(),
            transaction_type,
            commitment_level: CommitmentLevel::Confirmed,
            do_await_confirmation: false,
            cheatcode_data: Some(cheatcode_data),
        }
    }

    pub fn create_temp_account(
        transaction: &Transaction,
        keypairs: Vec<&Keypair>,
        temp_authority_keypair: &Keypair,
        already_exists: bool,
    ) -> Self {
        Self::new(
            transaction,
            keypairs,
            Some(vec![TxtxDeploymentSigner::Payer]),
            DeploymentTransactionType::PrepareTempAuthority {
                keypair_bytes: temp_authority_keypair.to_bytes().to_vec(),
                already_exists,
            },
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn create_buffer(
        transaction: &Transaction,
        keypairs: Vec<&Keypair>,
        buffer_pubkey: Pubkey,
    ) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::CreateBuffer { buffer_pubkey },
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn resize_buffer(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::ResizeBuffer,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn create_buffer_and_extend_program(
        transaction: &Transaction,
        keypairs: Vec<&Keypair>,
        buffer_pubkey: Pubkey,
    ) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::CreateBufferAndExtendProgram { buffer_pubkey },
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn extend_program(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::ExtendProgram,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn write_to_buffer(
        transaction: &Transaction,
        keypairs: Vec<&Keypair>,
        commitment_level: CommitmentLevel,
        do_await_confirmation: bool,
        is_upgrade: bool,
    ) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::WriteToBuffer { is_upgrade },
            commitment_level,
            do_await_confirmation,
        )
    }

    pub fn transfer_buffer_authority(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::TransferBufferAuthority,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn transfer_program_authority(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::TransferProgramAuthority,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn deploy_program(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::DeployProgram,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    /// Builds a single-transaction token deployment. The `keypairs` are the ephemeral
    /// mint keypair(s) that must be co-signed at the same recent blockhash as the txtx
    /// signer(s); `signers: Some(..)` marks that a txtx signer (payer/authority) must
    /// also sign, so the signer re-signs the mint and signs the payer in one step at a
    /// freshly fetched blockhash.
    pub fn deploy_token(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            Some(vec![TxtxDeploymentSigner::Payer]),
            DeploymentTransactionType::DeployToken,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn upgrade_program(transaction: &Transaction, keypairs: Vec<&Keypair>) -> Self {
        Self::new(
            transaction,
            keypairs,
            Some(vec![TxtxDeploymentSigner::FinalAuthority]),
            DeploymentTransactionType::UpgradeProgram,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn cheatcode_deploy(authority_pubkey: Pubkey, binary: Vec<u8>) -> Self {
        Self::new_cheatcode_deployment(
            DeploymentTransactionType::CheatcodeDeployment,
            (authority_pubkey, binary),
        )
    }

    pub fn cheatcode_upgrade(authority_pubkey: Pubkey, binary: Vec<u8>) -> Self {
        Self::new_cheatcode_deployment(
            DeploymentTransactionType::CheatcodeUpgrade,
            (authority_pubkey, binary),
        )
    }

    pub fn temp_authority_close_temp_authority(
        transaction: &Transaction,
        keypairs: Vec<&Keypair>,
    ) -> Self {
        Self::new(
            transaction,
            keypairs,
            None,
            DeploymentTransactionType::CloseTempAuthority,
            CommitmentLevel::Confirmed,
            true,
        )
    }

    pub fn skip_temp_authority_close() -> Self {
        Self {
            signers: None,
            transaction: None,
            keypairs_bytes: vec![],
            transaction_type: DeploymentTransactionType::SkipCloseTempAuthority,
            commitment_level: CommitmentLevel::Confirmed,
            do_await_confirmation: false,
            cheatcode_data: None,
        }
    }

    pub fn to_value(&self) -> Result<Value, Diagnostic> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| diagnosed_error!("failed to serialize transaction with keypairs: {e}"))?;
        Ok(SvmValue::deployment_transaction(bytes))
    }

    pub fn from_value(value: &Value) -> Result<Self, Diagnostic> {
        let addon_data = value.as_addon_data().ok_or(diagnosed_error!(
            "expected addon data for deployment transaction, found: {}",
            value.get_type().to_string()
        ))?;
        if addon_data.id == SVM_DEPLOYMENT_TRANSACTION {
            return serde_json::from_slice(&addon_data.bytes).map_err(|e| {
                diagnosed_error!("failed to deserialize deployment transaction: {e}")
            });
        } else if addon_data.id == SVM_CLOSE_TEMP_AUTHORITY_TRANSACTION_PARTS {
            let parts = CloseTempAuthorityTransactionParts::from_value(value)?;
            return match UpgradeableProgramDeployer::get_close_temp_authority_transaction(&parts)? {
                Some(transaction_value) => Self::from_value(&transaction_value),
                None => Ok(Self::skip_temp_authority_close()),
            };
        } else {
            return Err(diagnosed_error!(
                "failed to decode deployment transaction: invalid addon data type: {}",
                addon_data.id
            ));
        }
    }

    /// Converts a [Value] containing a [DeploymentTransaction] into a [DeploymentTransactionType].
    /// If you have a transaction [Value] and just need the inner type, this method is preferred to [DeploymentTransaction::from_value],
    /// as it avoids the rpc requests to fully assemble a deployment transaction from [CloseTempAuthorityTransactionParts].
    pub fn transaction_type_from_value(
        value: &Value,
    ) -> Result<DeploymentTransactionType, Diagnostic> {
        let addon_data = value.as_addon_data().ok_or(diagnosed_error!(
            "expected addon data for deployment transaction, found: {}",
            value.get_type().to_string()
        ))?;
        if addon_data.id == SVM_DEPLOYMENT_TRANSACTION {
            let deployment_tx: DeploymentTransaction = serde_json::from_slice(&addon_data.bytes)
                .map_err(|e| {
                    diagnosed_error!("failed to deserialize deployment transaction: {e}")
                })?;
            return Ok(deployment_tx.transaction_type);
        } else if addon_data.id == SVM_CLOSE_TEMP_AUTHORITY_TRANSACTION_PARTS {
            return Ok(DeploymentTransactionType::CloseTempAuthority);
        } else {
            return Err(diagnosed_error!(
                "failed to decode deployment transaction: invalid addon data type: {}",
                addon_data.id
            ));
        }
    }

    pub fn get_signers_dids(
        &self,
        authority_signer_did: ConstructDid,
        payer_signer_did: ConstructDid,
    ) -> Result<Option<Vec<ConstructDid>>, Diagnostic> {
        let signer_dids = match &self.signers {
            Some(signers) => Some(
                signers
                    .iter()
                    .filter_map(|s| {
                        if s.to_signer_key() == AUTHORITY {
                            Some(authority_signer_did.clone())
                        } else if s.to_signer_key() == PAYER {
                            Some(payer_signer_did.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<ConstructDid>>(),
            ),
            None => None,
        };

        Ok(signer_dids)
    }

    pub fn get_formatted_transaction(
        &self,
        signer_dids: Vec<ConstructDid>,
        signers_instances: &HashMap<ConstructDid, SignerInstance>,
    ) -> Result<Option<(Value, String)>, Diagnostic> {
        let meta_description = match &self.transaction_type {
            DeploymentTransactionType::PrepareTempAuthority { already_exists, .. } => {
                if *already_exists {
                    "This transaction funds the provided ephemeral account."
                } else {
                    "This transaction creates an ephemeral account that will write to the program buffer. This reduces the number of manual signatures required."
                }
            }
            DeploymentTransactionType::CreateBuffer { .. } => return Ok(None),
            DeploymentTransactionType::CreateBufferAndExtendProgram { .. } => return Ok(None),
            DeploymentTransactionType::ExtendProgram => return Ok(None),
            DeploymentTransactionType::WriteToBuffer { .. } => return Ok(None),
            DeploymentTransactionType::TransferBufferAuthority => return Ok(None),
            DeploymentTransactionType::TransferProgramAuthority => return Ok(None),
            DeploymentTransactionType::DeployProgram => "This transaction will deploy the program.",
            DeploymentTransactionType::DeployToken => {
                "This transaction will create and initialize the token mint."
            }
            DeploymentTransactionType::UpgradeProgram => {
                "This transaction will upgrade the program."
            }
            DeploymentTransactionType::CloseTempAuthority => return Ok(None),
            DeploymentTransactionType::SkipCloseTempAuthority => return Ok(None),
            DeploymentTransactionType::CheatcodeDeployment => return Ok(None),
            DeploymentTransactionType::CheatcodeUpgrade => return Ok(None),
            DeploymentTransactionType::ResizeBuffer => return Ok(None),
        };

        let meta_description = get_formatted_transaction_meta_description(
            &vec![meta_description.to_string()],
            &signer_dids,
            signers_instances,
        );

        let formatted_transaction =
            message_to_formatted_tx(&self.transaction.as_ref().unwrap().message);

        Ok(Some((formatted_transaction, meta_description)))
    }

    pub fn get_keypairs(&self) -> Result<Vec<Keypair>, Diagnostic> {
        self.keypairs_bytes
            .iter()
            .map(|b| Keypair::try_from(b.as_ref()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| diagnosed_error!("failed to decode keypair: {e}"))
    }

    pub fn sign_transaction_with_keypairs(
        &self,
        rpc_api_url: &str,
    ) -> Result<Transaction, Diagnostic> {
        let rpc_client = RpcClient::new_with_commitment(
            rpc_api_url,
            CommitmentConfig { commitment: self.commitment_level },
        );

        let blockhash = rpc_client
            .get_latest_blockhash()
            .map_err(|e| diagnosed_error!("failed to get latest blockhash: {e}"))?;

        let mut transaction: Transaction = self.transaction.as_ref().unwrap().clone();

        transaction.message.recent_blockhash = blockhash;
        let keypairs =
            self.get_keypairs().map_err(|e| diagnosed_error!("failed to sign transaction: {e}"))?;

        transaction
            .try_partial_sign(&keypairs, transaction.message.recent_blockhash)
            .map_err(|e| diagnosed_error!("failed to sign transaction: {e}"))?;

        Ok(transaction)
    }

    pub fn pre_send_status_updates(
        &self,
        logger: &LogDispatcher,
        transaction_index: usize,
        transaction_count: usize,
    ) -> Result<(), Diagnostic> {
        match &self.transaction_type {
            DeploymentTransactionType::SkipCloseTempAuthority => {
                logger.info(
                    "Ephemeral Account Closed",
                    format!(
                    "Ephemeral authority account has no leftover funds; skipping transaction to close the account",
                ));
                return Ok(());
            }
            DeploymentTransactionType::PrepareTempAuthority {
                keypair_bytes: temp_authority_keypair_bytes,
                already_exists,
            } => {
                let temp_authority_keypair = Keypair::try_from(temp_authority_keypair_bytes.as_ref())
                    .map_err(|e| {
                        diagnosed_error!("failed to deserialize temp authority keypair: {}", e)
                    })?;
                if *already_exists {
                    logger.info(
                        "[Recovery Instructions]",
                        format!(
                            "Using the provided Ephemeral authority with pubkey: {}",
                            temp_authority_keypair.pubkey()
                        ),
                    );
                } else {
                    logger
                    .info("[Recovery Instructions]","An ephemeral authority account will be created and funded to write to the buffer account.");
                    logger
                    .info("[Recovery Instructions]","Please save the following information in case the deployment fails and the account needs to be recovered:");
                    logger.info(
                        "[Recovery Instructions]",
                        format!(
                            "Ephemeral authority public key: {}",
                            temp_authority_keypair.pubkey()
                        ),
                    );
                    logger.info(
                        "[Recovery Instructions]",
                        format!(
                            "Ephemeral authority secret key: {:?}",
                            temp_authority_keypair.to_bytes()
                        ),
                    );
                }
            }
            DeploymentTransactionType::CreateBuffer { buffer_pubkey }
            | DeploymentTransactionType::CreateBufferAndExtendProgram { buffer_pubkey } => {
                logger.info(
                    "[Recovery Instructions]",
                    format!("Creating program buffer account at pubkey {}", buffer_pubkey),
                );
            }
            _ => {}
        };

        if match &self.transaction_type {
            DeploymentTransactionType::CheatcodeDeployment
            | DeploymentTransactionType::CheatcodeUpgrade => false,
            _ => true,
        } {
            logger.pending_info(
                "Pending",
                &format!("Sending transaction {}/{}", transaction_index + 1, transaction_count),
            );
        }

        Ok(())
    }

    pub fn post_send_status_updates(&self, logger: &LogDispatcher, program_id: Pubkey) {
        match self.transaction_type {
            DeploymentTransactionType::PrepareTempAuthority { already_exists, .. } => {
                logger.info(
                    format!("Account {}", if already_exists { "Funded" } else { "Created" }),
                    format!(
                        "Ephemeral authority account{} funded to write to buffer",
                        if already_exists { "" } else { " created and" }
                    ),
                );
            }
            DeploymentTransactionType::CreateBuffer { .. } => {
                logger.info("Account Created", "Program buffer account created");
            }
            DeploymentTransactionType::DeployProgram => {
                logger.info("Program Created", format!("Program {} has been deployed", program_id));
            }
            DeploymentTransactionType::UpgradeProgram => {
                logger
                    .info("Program Upgraded", format!("Program {} has been upgraded", program_id));
            }
            DeploymentTransactionType::CloseTempAuthority => {
                logger.success_info(
                    "Complete",
                    "Ephemeral authority account closed and leftover funds returned to payer",
                );
            }
            DeploymentTransactionType::CheatcodeDeployment => {
                logger.info("Program Created", format!("Program {} has been deployed", program_id));
            }
            DeploymentTransactionType::CheatcodeUpgrade => {
                logger
                    .info("Program Upgraded", format!("Program {} has been upgraded", program_id));
            }
            DeploymentTransactionType::WriteToBuffer { is_upgrade } =>
            // if it's a buffer write and do_await_confirmation=true, this is our last buffer write tx
            {
                if self.do_await_confirmation {
                    if is_upgrade {
                        // if this is an upgrade, we have another transaction to sign next, so we'll end the
                        // "pending" message spinner
                        logger.success_info("Buffer Ready", "Writing to buffer account is complete")
                    } else {
                        // if not upgrade, we can keep the "pending" spinner running - the next transaction isn't signed by the user
                        logger.info("Buffer Ready", "Writing to buffer account is complete")
                    }
                }
            }
            DeploymentTransactionType::TransferBufferAuthority => {
                logger.info(
                    "Buffer Authority Transferred",
                    "Buffer authority has been transferred to authority signer",
                );
            }
            _ => {}
        }
    }

    pub fn post_send_actions(&self, rpc_api_url: &str) {
        match self.transaction_type {
            // We want to avoid more than one transaction impacting the program account in a single slot
            // (because the bpf program throws if so), so after the extend program tx we'll wait one slot before continuing
            DeploymentTransactionType::ExtendProgram
            | DeploymentTransactionType::CreateBufferAndExtendProgram { .. } => {
                let rpc_client = RpcClient::new(rpc_api_url.to_string());
                wait_n_slots(&rpc_client, 1);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CloseTempAuthorityTransactionParts {
    pub temp_authority_keypair_bytes: Vec<u8>,
    pub rpc_api_url: String,
    pub payer_pubkey: Pubkey,
}

impl CloseTempAuthorityTransactionParts {
    pub fn to_value(&self) -> Result<Value, Diagnostic> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| diagnosed_error!("failed to serialize close temp authority parts: {e}"))?;
        Ok(SvmValue::close_temp_authority_transaction_parts(bytes))
    }
    pub fn from_value(value: &Value) -> Result<Self, Diagnostic> {
        let addon_data = value
            .as_addon_data()
            .ok_or(diagnosed_error!("expected addon data for close temp authority parts"))?;

        if addon_data.id == SVM_CLOSE_TEMP_AUTHORITY_TRANSACTION_PARTS {
            return serde_json::from_slice(&addon_data.bytes).map_err(|e| {
                diagnosed_error!("failed to deserialize close temp authority parts: {e}")
            });
        } else {
            return Err(diagnosed_error!("expected close temp authority parts"));
        }
    }
}

/// A struct to help deploy and upgrade an upgradeable program.
///
/// ### Actors
///  - Final Authority: this is who we want to own the program and program data account at the end of each deployment/upgrade.
///  - Ephemeral authority: this is a temporary keypair that we use to write to the buffer account.
///    - This prevents the user from having to sign many transactions to deploy/upgrade.
///    - Once this account is created/funded, we need to be sure to display the keys to the user. If the deployment fails, we don't want them to lose this account
///  - Buffer account: this is the account that is created to temporarily write data to before deploying to the final program address
///  - Payer: this is who pays to fund the temp authority
///
/// ## First Deployment
///  
/// ### Transaction 1: Seed Ephemeral authority
///  1. This is signed by the payer
///
/// ### Transaction 2: Create Buffer
///  1. First instruction creates the Buffer account
///     1. Signed by the Ephemeral authority
///  2. Second instruction initializes the Buffer, with the Ephemeral authority as the authority
///
/// ### Transaction 3 - X: Write to Buffer
///  1. Ephemeral authority writes to Buffer
///
/// ### Transaction X + 1: Deploy Program              
///  1. Create final program account
///     1. Ephemeral authority signs
///  2. Transfer buffer to final program
///     1. Ephemeral authority signs (Buffer authority **must match** program authority)
///     2. After this, the Ephemeral authority owns the final program
///
/// ### Transaction X + 2: Transfer Program authority from Ephemeral authority to Final Authority
///  1. Ephemeral authority signs
///
/// ### Transaction X + 3: Transfer leftover Ephemeral authority funds to the Payer
///  1. Ephemeral authority signs
/// ---
///
/// ## Upgrades
///
/// ### Transaction 1: Seed Ephemeral authority
///  1. This is signed by the payer
///
/// ### Transaction 2: Create Buffer
///  1. First instruction creates the Buffer account
///     1. Signed by the Ephemeral authority
///  2. Second instruction initializes the Buffer, with the Ephemeral authority as the authority
///  3. Third instruction extends the program data account if necessary
///     1. Payed for by the Ephemeral authority
///
/// ### Transaction 3 - X: Write to Buffer
///  1. Ephemeral authority writes to Buffer
///
/// ### Transaction X + 1: Transfer Buffer authority from Ephemeral authority to Final Authority
///  1. Ephemeral authority signs
///
/// ### Transaction X + 2: Upgrade Program              
///  1. Final Authority signs
///
/// ### Transaction X + 3: Transfer leftover Ephemeral authority funds to the Payer
///  1. Ephemeral authority signs
///
pub struct UpgradeableProgramDeployer {
    /// The public key of the program to deploy.
    pub program_pubkey: Pubkey,
    /// The keypair of the program to deploy.
    pub program_keypair: Option<Keypair>,
    /// The public key of the payer.
    pub payer_pubkey: Pubkey,
    /// The public key of the final upgrade authority. (Can be the same as the payer)
    pub final_upgrade_authority_pubkey: Pubkey,
    /// The pubkey of the temporary upgrade authority.
    pub temp_upgrade_authority_pubkey: Pubkey,
    /// The keypair of the temporary upgrade authority.
    pub temp_upgrade_authority: Keypair,
    /// The public key of the buffer account. The buffer account exists to be a temporary address where the program is deployed.
    /// If there are failures in the deployment, the same buffer account can be provided to retry the deployment.
    pub buffer_pubkey: Pubkey,
    /// The keypair of the buffer account.
    pub buffer_keypair: Option<Keypair>,
    /// The data of the buffer account from the previous deployment attempt.
    pub buffer_data: Vec<u8>,
    /// The binary of the program to deploy.
    pub binary: Vec<u8>,
    /// The RPC client to use for fetching the latest blockhash and minimum balance for rent exemption.
    pub rpc_client: RpcClient,
    /// Whether to auto extend the program data account if it is too small to accommodate the new program.
    pub auto_extend: bool,
    /// Whether the program is being upgraded (true), or deployed for the first time (false).
    pub is_program_upgrade: bool,
    /// Whether the underlying network is a Surfnet.
    pub is_surfnet: bool,
    /// Whether to perform a hot swap of the program deployment (using surfnet cheatcodes).
    pub do_cheatcode_deploy: bool,
}

pub enum KeypairOrTxSigner {
    Keypair(Keypair),
    TxSigner(Pubkey),
}

impl UpgradeableProgramDeployer {
    /// Creates a new instance with the provided parameters.
    ///
    /// # Parameters
    ///
    /// * `program_keypair` - The keypair for the program being deployed.
    /// * `upgrade_authority_keypair` - The keypair for the upgrade authority. (Can be the same as the payer)
    /// * `binary` - A reference to a vector of bytes representing the binary data.
    /// * `payer_pubkey` - The public key of the payer.
    /// * `rpc_client` - The RPC client to interact with the Solana network.
    /// * `commitment` - An optional commitment configuration. If `None`, the default commitment level is `Confirmed`.
    /// * `existing_program_buffer_opts` - An optional tuple containing:
    ///     * `Pubkey` - The public key of the existing program buffer.
    ///     * `Keypair` - The keypair associated with the existing program buffer.
    ///     * `Vec<u8>` - A vector of bytes representing the existing program buffer data. If `None`, a new program buffer will be created.
    pub fn new(
        program_pubkey: Pubkey,
        program_keypair: Option<Keypair>,
        final_upgrade_authority_pubkey: &Pubkey,
        temp_authority_keypair: Keypair,
        binary: &Vec<u8>,
        payer_pubkey: &Pubkey,
        rpc_client: RpcClient,
        existing_program_buffer_opts: Option<Pubkey>,
        auto_extend: Option<bool>,
        is_surfnet: bool,
        hot_swap: bool,
    ) -> Result<Self, Diagnostic> {
        let (buffer_pubkey, buffer_keypair, buffer_data) = match existing_program_buffer_opts {
            Some(buffer_pubkey) => {
                let buffer_account = rpc_client.get_account_with_commitment(&buffer_pubkey, CommitmentConfig::processed()).map_err(|e| {
                    diagnosed_error!(
                        "failed to fetch existing buffer account {}: {}",
                        buffer_pubkey,
                        e
                    )
                })?.value.ok_or(diagnosed_error!(
                    "existing buffer account {} does not exist",
                    buffer_pubkey
                ))?;

                if buffer_account.owner != solana_sdk_ids::bpf_loader_upgradeable::id() {
                    return Err(diagnosed_error!(
                        "buffer account {} is not owned by the bpf_loader_upgradeable program",
                        buffer_pubkey
                    ));
                }

                let min_buffer_data_len = UpgradeableLoaderState::size_of_buffer(binary.len());
                if buffer_account.data.len() < min_buffer_data_len {
                    return Err(diagnosed_error!(
                            "existing buffer account {} data size ({} bytes) is too small for the program binary ({} bytes)",
                            buffer_pubkey,
                            buffer_account.data.len(),
                            min_buffer_data_len
                        ));
                }

                let mut cursor = std::io::Cursor::new(&buffer_account.data);
                // Deserialize only the prefix into UpgradeableLoaderState
                let state: UpgradeableLoaderState =
                    bincode::deserialize_from(&mut cursor).map_err(|e| e.to_string())?;
                // Figure out how many bytes we consumed
                let state_len = cursor.position() as usize;

                // The rest is the ELF program data
                let program_bytes = &buffer_account.data[state_len..];

                let (authority_address, program_bytes) = match state {
                    UpgradeableLoaderState::Buffer { authority_address } => {
                        (authority_address, program_bytes)
                    }
                    _ => {
                        return Err(diagnosed_error!(
                            "provided buffer pubkey {} is not a buffer account",
                            buffer_pubkey
                        ))
                    }
                };

                let Some(authority_address) = authority_address else {
                    return Err(diagnosed_error!(
                        "buffer account {} has no authority set, so it can't be written to",
                        buffer_pubkey
                    ));
                };

                if authority_address != temp_authority_keypair.pubkey() {
                    return Err(diagnosed_error!(
                        "buffer account {} authority does not match the provided temp authority pubkey",
                        buffer_pubkey
                    ));
                }
                (buffer_pubkey, None, program_bytes.to_vec())
            }
            None => {
                let (_buffer_words, _buffer_mnemonic, buffer_keypair) = create_ephemeral_keypair();
                (buffer_keypair.pubkey(), Some(buffer_keypair), vec![0; binary.len()])
            }
        };
        let do_cheatcode_deploy = hot_swap && is_surfnet;

        let is_program_upgrade = !UpgradeableProgramDeployer::should_do_initial_deploy(
            &rpc_client,
            &program_pubkey,
            &final_upgrade_authority_pubkey,
            do_cheatcode_deploy
        )?;

        Ok(Self {
            program_pubkey,
            program_keypair,
            final_upgrade_authority_pubkey: *final_upgrade_authority_pubkey,
            temp_upgrade_authority_pubkey: temp_authority_keypair.pubkey(),
            temp_upgrade_authority: temp_authority_keypair,
            binary: binary.clone(),
            payer_pubkey: *payer_pubkey,
            rpc_client,
            buffer_keypair,
            buffer_pubkey,
            buffer_data,
            auto_extend: auto_extend.unwrap_or(true),
            is_program_upgrade,
            is_surfnet,
            do_cheatcode_deploy,
        })
    }

    fn get_core_transactions_internal(&mut self, recent_blockhash: &Hash) -> Result<Vec<Value>, Diagnostic> {
        
        let mut core_transactions =
            // transactions for first deployment of a program
            if !self.is_program_upgrade {


                if self.do_cheatcode_deploy {
                    vec![DeploymentTransaction::cheatcode_deploy(self.final_upgrade_authority_pubkey.clone(), self.binary.clone()).to_value()?]
                } else {
                    let Some(keypair) = self.program_keypair.as_ref() else {
                        return Err(diagnosed_error!("program keypair is required for initial deployment; does your `target/deploy` folder have a keypair.json?"));
                    };

                    if keypair.pubkey() != self.program_pubkey {
                        return Err(diagnosed_error!(
                            "program keypair does not match program pubkey found in IDL: keypair pubkey: '{}'; IDL pubkey: '{}'",
                            keypair.pubkey(),
                            self.program_pubkey
                        ));
                    }
                    
                    // create the buffer account
                    let create_buffer_account_transaction =
                        self.get_create_buffer_transaction(&recent_blockhash)?;

                    // write transaction data to the buffer account
                    let mut write_transactions = self.get_write_to_buffer_transactions(&recent_blockhash)?;


                    let mut transactions = vec![];
                    if let Some(create_buffer_transaction) = create_buffer_account_transaction {
                        transactions.push(create_buffer_transaction);
                    }
                    transactions.append(&mut write_transactions);
                    transactions
                }
            }
            // transactions for upgrading an existing program
            else {
                if self.do_cheatcode_deploy {
                    vec![DeploymentTransaction::cheatcode_upgrade(self.final_upgrade_authority_pubkey.clone(), self.binary.clone()).to_value()?]
                } else {
                    // extend the program length and create the buffer account
                    let prepare_program_upgrade_transaction =
                        self.get_prepare_program_upgrade_transaction(&recent_blockhash)?;

                    // write transaction data to the buffer account 
                    let mut write_transactions = self.get_write_to_buffer_transactions(&recent_blockhash)?;


                    let mut transactions = vec![];
                    if let Some(prepare_program_upgrade_transaction) = prepare_program_upgrade_transaction {
                        transactions.push(prepare_program_upgrade_transaction);
                    }
                    transactions.append(&mut write_transactions);
                    transactions
                }
            };

        let transactions = if self.do_cheatcode_deploy {
            core_transactions
        } else {
            let mut transactions = vec![];
            // the first transaction needs to create the temp account
            if let Some(create_temp_account_transaction) = self
                .get_create_temp_account_transaction(&recent_blockhash, core_transactions.len())?
            {
                transactions.push(create_temp_account_transaction);
            }
            transactions.append(&mut core_transactions);
            transactions
        };
        Ok(transactions)
    }

    pub fn get_cleanup_transactions_count() -> usize {
        3 // finalize + transfer authority + close temp authority
    }
    
    fn get_cleanup_transactions(&self, recent_blockhash: &Hash) -> Result<Vec<Value>, Diagnostic> {
        if self.do_cheatcode_deploy {
            Ok(vec![])
        } else {
            let mut cleanup_transactions = if !self.is_program_upgrade {

                        // deploy the program, with the final authority as program authority
                        let finalize_transaction =
                            self.get_deploy_with_max_program_len_transaction(&recent_blockhash)?;

                        // transfer the program authority from the temp authority to the final authority
                        let transfer_authority = self.get_set_program_authority_to_final_authority_transaction(&recent_blockhash)?;
                        vec![finalize_transaction, transfer_authority]
            }
            else {
                        // transfer the buffer authority from the temp authority to the final authority
                        let transfer_authority = self.get_set_buffer_authority_to_final_authority_transaction(&recent_blockhash)?;

                        // upgrade the program, with the final authority as program authority
                        let upgrade_transaction = self.get_upgrade_transaction(&recent_blockhash)?;

                        vec![transfer_authority, upgrade_transaction]
            };
            cleanup_transactions.push(self.get_close_temp_authority_transaction_parts()?);
            Ok(cleanup_transactions)
        }
    }

    
    
    pub fn get_all_transactions(&mut self) -> Result<Vec<Value>, Diagnostic> {
        let recent_blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .map_err(|e| diagnosed_error!("failed to fetch latest blockhash: rpc error: {e}"))?;

        let transactions = {
            let mut core_transactions = self.get_core_transactions_internal(&recent_blockhash)?;

            let mut cleanup_transactions = self.get_cleanup_transactions(&recent_blockhash)?;

            core_transactions.append(&mut cleanup_transactions);
            core_transactions
        };

        Ok(transactions)
    }

    pub fn get_core_transactions(&mut self) -> Result<Vec<Value>, Diagnostic> {
        let recent_blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .map_err(|e| diagnosed_error!("failed to fetch latest blockhash: rpc error: {e}"))?;

        let transactions = self.get_core_transactions_internal(&recent_blockhash)?;

        Ok(transactions)
    }

    fn get_create_temp_account_transaction(
        &self,
        blockhash: &Hash,
        transaction_count: usize,
    ) -> Result<Option<Value>, Diagnostic> {
        let mut lamports = 0;

        // calculate transaction fees for all deployment transactions
        {
            let buffer_create_tx_count = 1;
            let set_buffer_authority_tx_count = 1;
            let finalize_tx_count = 1;
            let write_tx_count = transaction_count
                - buffer_create_tx_count
                - set_buffer_authority_tx_count
                - finalize_tx_count;
            let return_funds_tx_count = 1;

            // let temp_account = self
            //     .rpc_client
            //     .get_account(&self.temp_upgrade_authority_pubkey)
            //     .map_err(|e| format!("failed to get account: {e}"))?;
            // lamports += lamports_per_signature_of(&AccountSharedData::from(temp_account))
            //     .unwrap_or(LAMPORTS_PER_SIGNATURE)
            //     * (buffer_create_tx_count
            //         + write_tx_count
            //         + set_buffer_authority_tx_count
            //         + return_funds_tx_count) as u64;
            lamports += LAMPORTS_PER_SIGNATURE
                * (buffer_create_tx_count
                    + write_tx_count
                    + set_buffer_authority_tx_count
                    + return_funds_tx_count) as u64;

            // let final_authority_account = self
            //     .rpc_client
            //     .get_account(&self.final_upgrade_authority_pubkey)
            //     .map_err(|e| format!("failed to get account: {e}"))?;
            // lamports +=
            //     lamports_per_signature_of(&AccountSharedData::from(final_authority_account))
            //         .unwrap_or(LAMPORTS_PER_SIGNATURE)
            //         * (finalize_tx_count) as u64;
            lamports += LAMPORTS_PER_SIGNATURE * (finalize_tx_count) as u64;
        }

        // calculate rent for all program data written
        {
            let program_data_length = self.binary.len();
            let program_data_rent_lamports = self
                .rpc_client
                .get_minimum_balance_for_rent_exemption(
                    UpgradeableLoaderState::size_of_programdata(program_data_length),
                )
                .unwrap();

            let buffer_account_lamports =
                self.rpc_client.get_account(&self.buffer_pubkey).map(|a| a.lamports).unwrap_or(0);
            // size for the program data. this data will be written to the buffer account first, so
            // only pay lamports for the needed lamports for rent - buffer_account_lamports
            lamports += program_data_rent_lamports.saturating_sub(buffer_account_lamports);

            // if this is a program upgrade, we also need to add in rent lamports for extending the program data account
            if self.is_program_upgrade {
                if let Some(additional_bytes) = self.get_extend_program_additional_bytes()? {
                    lamports += self
                        .rpc_client
                        .get_minimum_balance_for_rent_exemption(
                            UpgradeableLoaderState::size_of_programdata(additional_bytes as usize),
                        )
                        .unwrap();
                }
            }
        }

        // add 20% buffer
        let lamports = ((lamports as f64) * 1.2).round() as u64;

        let existing_account_lamports = self
            .rpc_client
            .get_account(&self.temp_upgrade_authority_pubkey)
            .map(|a| a.lamports)
            .ok();

        let (instruction, keypairs) =
            if let Some(existing_account_lamports) = existing_account_lamports {
                if existing_account_lamports >= lamports {
                    return Ok(None);
                } else {
                    (
                        system_instruction::transfer(
                            &self.payer_pubkey,
                            &self.temp_upgrade_authority_pubkey,
                            lamports - existing_account_lamports,
                        ),
                        vec![],
                    )
                }
            } else {
                (
                    system_instruction::create_account(
                        &self.payer_pubkey,
                        &self.temp_upgrade_authority_pubkey,
                        lamports,
                        0,
                        &solana_sdk_ids::system_program::id(),
                    ),
                    vec![&self.temp_upgrade_authority],
                )
            };

        let message =
            Message::new_with_blockhash(&[instruction], Some(&self.payer_pubkey), &blockhash);

        let transaction = Transaction::new_unsigned(message);

        Ok(Some(
            DeploymentTransaction::create_temp_account(
                &transaction,
                keypairs,
                &self.temp_upgrade_authority,
                existing_account_lamports.is_some(),
            )
            .to_value()?,
        ))
    }

    fn get_create_buffer_instruction(&self) -> Result<Vec<Instruction>, Diagnostic> {
        let program_data_length = self.binary.len();

        let rent_lamports = self
            .rpc_client
            .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_programdata(
                program_data_length,
            ))
            .map_err(|e| {
                diagnosed_error!("failed to get minimum balance for rent exemption: {e}")
            })?;

        create_buffer(
            &self.temp_upgrade_authority_pubkey,
            &self.buffer_pubkey,
            &self.temp_upgrade_authority_pubkey,
            rent_lamports,
            program_data_length,
        )
        .map_err(|e| diagnosed_error!("failed to create buffer: {e}"))
    }

    fn get_create_buffer_transaction(&self, blockhash: &Hash) -> Result<Option<Value>, Diagnostic> {
        if let Some(buffer_keypair) = self.buffer_keypair.as_ref() {
            if self
                .rpc_client
                .get_account(&self.buffer_pubkey).ok().is_some()
            {
                return Ok(None);
            }
            let create_buffer_instruction = self.get_create_buffer_instruction()?;

            let message = Message::new_with_blockhash(
                &create_buffer_instruction,
                Some(&self.temp_upgrade_authority_pubkey), // todo: can this be none? isn't the payer already set in the instruction
                &blockhash,
            );

            let transaction = Transaction::new_unsigned(message);

            let tx = DeploymentTransaction::create_buffer(
                &transaction,
                vec![&self.temp_upgrade_authority, &buffer_keypair],
                buffer_keypair.pubkey(),
            )
            .to_value()?;
            Ok(Some(tx))
        } else {
            Ok(None)
        }
    }

    // Mostly copied from solana cli: https://github.com/txtx/solana/blob/8116c10021f09c806159852f65d37ffe6d5a118e/cli/src/program.rs#L2807
    fn get_extend_program_instruction(&self) -> Result<Option<Instruction>, Diagnostic> {
        let some_instruction =
            if let Some(additional_bytes) = self.get_extend_program_additional_bytes()? {
                let instruction = bpf_loader_upgradeable::extend_program(
                    &self.program_pubkey,
                    Some(&self.temp_upgrade_authority_pubkey),
                    additional_bytes,
                );
                Some(instruction)
            } else {
                None
            };
        Ok(some_instruction)
    }

    fn get_extend_program_additional_bytes(&self) -> Result<Option<u32>, Diagnostic> {
        let program_data_address = get_program_data_address(&self.program_pubkey);

        let Some(program_data_account) = self
            .rpc_client
            .get_account_with_commitment(&program_data_address, CommitmentConfig::processed())
            .map_err(|e| diagnosed_error!("failed to get program data account: {e}",))?
            .value
        else {
            // Program data has not been allocated yet.
            return Ok(None);
        };

        let program_len = self.binary.len();
        let required_len = UpgradeableLoaderState::size_of_programdata(program_len);
        let max_permitted_data_length = usize::try_from(MAX_PERMITTED_DATA_LENGTH).unwrap();
        if required_len > max_permitted_data_length {
            let max_program_len = max_permitted_data_length
                .saturating_sub(UpgradeableLoaderState::size_of_programdata(0));
            return Err(diagnosed_error!(
                "New program ({}) data account is too big: {}.\n\
             Maximum program size: {}.",
                &self.program_pubkey,
                required_len,
                max_program_len
            )
            .into());
        }

        let current_len = program_data_account.data.len();
        let additional_bytes = required_len.saturating_sub(current_len);
        if additional_bytes == 0 {
            // Current allocation is sufficient.
            return Ok(None);
        }

        let additional_bytes =
            u32::try_from(additional_bytes).expect("`u32` is big enough to hold an account size");
        Ok(Some(additional_bytes))
    }

    fn get_prepare_program_upgrade_transaction(
        &self,
        blockhash: &Hash,
    ) -> Result<Option<Value>, Diagnostic> {
        let create_buffer_instructions = if self.buffer_keypair.is_some() {
            let create_buffer_instructions = self.get_create_buffer_instruction()?;
            Some(create_buffer_instructions)
        } else {
            None
        };

        let extend_program_instruction = if self.auto_extend {
            if let Some(extend_program_instruction) = self.get_extend_program_instruction()? {
                Some(extend_program_instruction)
            } else {
                None
            }
        } else {
            None
        };

        fn prep_tx(ixs: &[Instruction], blockhash: &Hash, payer: &Pubkey) -> Transaction {
            let message = Message::new_with_blockhash(ixs, Some(payer), blockhash);
            Transaction::new_unsigned(message)
        }

        let tx = match (create_buffer_instructions, extend_program_instruction) {
            (Some(mut create_buffer_instructions), Some(extend_program_instruction)) => {
                create_buffer_instructions.push(extend_program_instruction);
                let transaction = prep_tx(
                    &create_buffer_instructions,
                    &blockhash,
                    &self.temp_upgrade_authority_pubkey,
                );
                DeploymentTransaction::create_buffer_and_extend_program(
                    &transaction,
                    vec![&self.temp_upgrade_authority, self.buffer_keypair.as_ref().unwrap()],
                    self.buffer_keypair.as_ref().unwrap().pubkey(),
                )
                .to_value()?
            }
            (Some(create_buffer_instructions), None) => {
                let transaction = prep_tx(
                    &create_buffer_instructions,
                    &blockhash,
                    &self.temp_upgrade_authority_pubkey,
                );
                DeploymentTransaction::create_buffer(
                    &transaction,
                    vec![&self.temp_upgrade_authority, self.buffer_keypair.as_ref().unwrap()],
                    self.buffer_keypair.as_ref().unwrap().pubkey(),
                )
                .to_value()?
            }
            (None, Some(extend_program_instruction)) => {
                let transaction = prep_tx(
                    &vec![extend_program_instruction],
                    &blockhash,
                    &self.temp_upgrade_authority_pubkey,
                );
                DeploymentTransaction::extend_program(
                    &transaction,
                    vec![&self.temp_upgrade_authority],
                )
                .to_value()?
            }
            (None, None) => {
                return Ok(None);
            }
        };

        Ok(Some(tx))
    }

    // Mostly copied from solana cli: https://github.com/txtx/solana/blob/8116c10021f09c806159852f65d37ffe6d5a118e/cli/src/program.rs#L2455
    fn get_write_to_buffer_transactions(&self, blockhash: &Hash) -> Result<Vec<Value>, Diagnostic> {
        let create_msg = |offset: u32, bytes: Vec<u8>| {
            let instruction = bpf_loader_upgradeable::write(
                &self.buffer_pubkey,
                &self.temp_upgrade_authority_pubkey,
                offset,
                bytes,
            );

            let instructions = vec![instruction];
            Message::new_with_blockhash(
                &instructions,
                Some(&self.temp_upgrade_authority_pubkey), // todo: can this be none? isn't the payer already set in the instruction
                &blockhash,
            )
        };

        let mut write_transactions = vec![];
        let chunk_size = calculate_max_chunk_size(&create_msg);

        let chunks = self.binary.chunks(chunk_size).collect::<Vec<_>>();

        for (chunk, i) in chunks.iter().zip(0usize..) {
            let offset = i.saturating_mul(chunk_size);

            let written_chunk = &self.buffer_data.get(offset..offset.saturating_add(chunk.len()));
            // Only write the chunk if it differs from our initial buffer data
            let do_write = written_chunk.is_none() || written_chunk.unwrap() != *chunk;
            if do_write {
                let transaction =
                    Transaction::new_unsigned(create_msg(offset as u32, chunk.to_vec()));

                let (do_await_confirmation, commitment_level) = if i == chunks.len() - 1 {
                    (true, CommitmentLevel::Confirmed)
                } else {
                    (false, CommitmentLevel::Processed)
                };

                write_transactions.push(
                    DeploymentTransaction::write_to_buffer(
                        &transaction,
                        vec![&self.temp_upgrade_authority],
                        commitment_level,
                        do_await_confirmation,
                        self.is_program_upgrade,
                    )
                    .to_value()?,
                );
            }
        }
        Ok(write_transactions)
    }

    fn get_set_buffer_authority_to_final_authority_transaction(
        &self,
        blockhash: &Hash,
    ) -> Result<Value, Diagnostic> {
        let instruction = bpf_loader_upgradeable::set_buffer_authority(
            &self.buffer_pubkey,
            &self.temp_upgrade_authority_pubkey,
            &self.final_upgrade_authority_pubkey,
        );

        let message = Message::new_with_blockhash(
            &[instruction],
            Some(&self.temp_upgrade_authority_pubkey), // todo: can this be none? isn't the payer already set in the instruction
            &blockhash,
        );

        let transaction = Transaction::new_unsigned(message);

        DeploymentTransaction::transfer_buffer_authority(
            &transaction,
            vec![&self.temp_upgrade_authority],
        )
        .to_value()
    }

    fn get_set_program_authority_to_final_authority_transaction(
        &self,
        blockhash: &Hash,
    ) -> Result<Value, Diagnostic> {
        let instruction = bpf_loader_upgradeable::set_upgrade_authority(
            &self.program_pubkey,
            &self.temp_upgrade_authority_pubkey,
            Some(&self.final_upgrade_authority_pubkey),
        );

        let message = Message::new_with_blockhash(
            &[instruction],
            Some(&self.temp_upgrade_authority_pubkey), // todo: can this be none? isn't the payer already set in the instruction
            &blockhash,
        );

        let transaction = Transaction::new_unsigned(message);

        DeploymentTransaction::transfer_program_authority(
            &transaction,
            vec![&self.temp_upgrade_authority],
        )
        .to_value()
    }

    fn get_close_temp_authority_transaction_parts(&self) -> Result<Value, Diagnostic> {
        CloseTempAuthorityTransactionParts {
            temp_authority_keypair_bytes: self.temp_upgrade_authority.to_bytes().to_vec(),
            rpc_api_url: self.rpc_client.url(),
            payer_pubkey: self.payer_pubkey,
        }
        .to_value()
    }

    /// Closing an account requires clearing out the data and removing all lamports to not cover rent.
    /// The temp account we're opening does not have any data, so we don't need to clear it.
    /// So this function is only used to send any leftover lamports back to the payer.
    pub fn get_close_temp_authority_transaction(
        CloseTempAuthorityTransactionParts {
            temp_authority_keypair_bytes,
            rpc_api_url,
            payer_pubkey,
        }: &CloseTempAuthorityTransactionParts,
    ) -> Result<Option<Value>, Diagnostic> {
        let temp_upgrade_authority_keypair =
            Keypair::try_from(temp_authority_keypair_bytes.as_ref()).unwrap();
        let temp_upgrade_authority_pubkey = temp_upgrade_authority_keypair.pubkey();

        // use processed commitment so we're sure to get the most recent balance
        let rpc_client = RpcClient::new_with_commitment(rpc_api_url, CommitmentConfig::confirmed());

        let err_prefix = format!(
            "failed to close temp upgrade authority account ({}) and send funds back to the payer",
            temp_upgrade_authority_pubkey
        );

        let blockhash = rpc_client.get_latest_blockhash().map_err(|e| {
            diagnosed_error!("{err_prefix}: failed to fetch latest blockhash: rpc error: {e}")
        })?;

        let err_prefix = format!(
            "failed to close temp upgrade authority account ({}) and send funds back to the payer",
            temp_upgrade_authority_pubkey
        );

        // fetch balance to know how much to transfer back
        let temp_authority_balance = rpc_client
            .get_balance(&temp_upgrade_authority_pubkey)
            .map_err(|e| diagnosed_error!("{err_prefix}: failed to get leftover balance: {e}"))?;

        if temp_authority_balance > (LAMPORTS_PER_SIGNATURE) {
            let instructions = vec![system_instruction::transfer(
                &temp_upgrade_authority_pubkey,
                &payer_pubkey,
                temp_authority_balance - (5000),
            )];

            let message = Message::new_with_blockhash(
                &instructions,
                Some(&temp_upgrade_authority_pubkey),
                &blockhash,
            );

            let transaction = Transaction::new_unsigned(message);

            return Some(
                DeploymentTransaction::temp_authority_close_temp_authority(
                    &transaction,
                    vec![&temp_upgrade_authority_keypair],
                )
                .to_value(),
            )
            .transpose();
        }

        return Ok(None);
    }

    fn get_deploy_with_max_program_len_transaction(
        &self,
        blockhash: &Hash,
    ) -> Result<Value, Diagnostic> {
        // @todo - deprecation warning on bpf_loader_upgradeable::deploy_with_max_program_len`: Use loader-v4 instead
        let instructions = bpf_loader_upgradeable::deploy_with_max_program_len(
            &self.temp_upgrade_authority_pubkey,
            &self.program_pubkey,
            &self.buffer_pubkey,
            &self.temp_upgrade_authority_pubkey,
            self.rpc_client
                .get_minimum_balance_for_rent_exemption(UpgradeableLoaderState::size_of_program())
                .map_err(|e| {
                    diagnosed_error!("failed to get minimum balance for rent exemption: {e}")
                })?,
            self.binary.len().max(self.buffer_data.len()),
        )
        .map_err(|e| {
            diagnosed_error!("failed to create deploy with max program len instruction: {e}")
        })?;

        let message = Message::new_with_blockhash(
            &instructions,
            Some(&self.temp_upgrade_authority_pubkey),
            &blockhash,
        );
        let transaction = Transaction::new_unsigned(message);

        DeploymentTransaction::deploy_program(
            &transaction,
            vec![&self.temp_upgrade_authority, &self.program_keypair.as_ref().unwrap()],
        )
        .to_value()
    }

    fn get_upgrade_transaction(&self, blockhash: &Hash) -> Result<Value, Diagnostic> {
        let upgrade_instruction = bpf_loader_upgradeable::upgrade(
            &self.program_pubkey,
            &self.buffer_pubkey,
            &self.final_upgrade_authority_pubkey,
            &self.payer_pubkey,
        );

        let message = Message::new_with_blockhash(
            &[upgrade_instruction],
            Some(&self.final_upgrade_authority_pubkey),
            &blockhash,
        );
        let transaction = Transaction::new_unsigned(message);

        DeploymentTransaction::upgrade_program(&transaction, vec![]).to_value()
    }

    pub fn check_is_surfnet(rpc_client: &RpcClient) -> Result<bool, Diagnostic> {
        let version = RpcVersionInfo::fetch_blocking(rpc_client)?;
        Ok(version.surfnet_version.is_some())
    }

    /// Logic mostly copied from solana cli: https://github.com/txtx/solana/blob/8116c10021f09c806159852f65d37ffe6d5a118e/cli/src/program.rs#L1248-L1249
    fn should_do_initial_deploy(
        rpc_client: &RpcClient,
        program_pubkey: &Pubkey,
        final_upgrade_authority_pubkey: &Pubkey,
        via_cheatcode: bool,
    ) -> Result<bool, Diagnostic> {
        if let Some(account) = rpc_client
            .get_account_with_commitment(&program_pubkey, CommitmentConfig::processed())
            .map_err(|e| diagnosed_error!("failed to get program account: {e}"))?
            .value
        {
            if account.owner != solana_sdk_ids::bpf_loader_upgradeable::id() {
                return Err(diagnosed_error!(
                    "Account {} is not an upgradeable program or already is in use",
                    program_pubkey
                )
                .into());
            }
            if !account.executable {
                return Ok(true);
            } else if let Ok(UpgradeableLoaderState::Program { programdata_address }) =
                account.state()
            {
                if let Some(account) = rpc_client
                    .get_account_with_commitment(
                        &programdata_address,
                        CommitmentConfig::processed(),
                    )
                    .map_err(|e| diagnosed_error!("failed to get program data account: {e}"))?
                    .value
                {
                    if let Ok(UpgradeableLoaderState::ProgramData {
                        slot: _,
                        upgrade_authority_address: program_authority_pubkey,
                    }) = account.state()
                    {
                        if let Some(program_authority_pubkey) = program_authority_pubkey {
                            if program_authority_pubkey != *final_upgrade_authority_pubkey {
                                // skip authority check if we're using the cheatcode deploy
                                if !via_cheatcode {
                                    return Err(diagnosed_error!(
                                        "Program's authority {:?} does not match authority provided {:?}",
                                        program_authority_pubkey, final_upgrade_authority_pubkey,
                                    )
                                    .into());
                                }
                            }
                        }
                        // Do upgrade
                        return Ok(false);
                    } else {
                        return Err(diagnosed_error!(
                            "Program {} has been closed, use a new Program Id",
                            program_pubkey
                        )
                        .into());
                    }
                } else {
                    return Err(diagnosed_error!(
                        "Program {} has been closed, use a new Program Id",
                        program_pubkey
                    )
                    .into());
                }
            } else {
                return Err(
                    diagnosed_error!("{} is not an upgradeable program", program_pubkey).into()
                );
            }
        } else {
            return Ok(true);
        };
    }

    // todo: need to make this function secure, and to verify the account doesn't already exist
    pub fn create_temp_authority() -> Keypair {
        let (_buffer_words, _buffer_mnemonic, temp_authority_keypair) = create_ephemeral_keypair();

        temp_authority_keypair
    }
}

// todo: need to make this function secure, and to verify the account doesn't already exist
fn create_ephemeral_keypair() -> (usize, Mnemonic, Keypair) {
    const WORDS: usize = 12;
    let mnemonic = Mnemonic::new(MnemonicType::for_word_count(WORDS).unwrap(), Language::English);
    let seed = Seed::new(&mnemonic, "");
    let new_keypair = keypair_from_seed(seed.as_bytes()).unwrap();
    (WORDS, mnemonic, new_keypair)
}

/// Copied from solana cli: https://github.com/txtx/solana/blob/8116c10021f09c806159852f65d37ffe6d5a118e/cli/src/program.rs#L2386
fn calculate_max_chunk_size<F>(create_msg: &F) -> usize
where
    F: Fn(u32, Vec<u8>) -> Message,
{
    let baseline_msg = create_msg(0, Vec::new());
    let tx_size = bincode::serialized_size(&Transaction {
        signatures: vec![
            Signature::default();
            baseline_msg.header.num_required_signatures as usize
        ],
        message: baseline_msg,
    })
    .unwrap() as usize;
    // add 1 byte buffer to account for shortvec encoding
    PACKET_DATA_SIZE.saturating_sub(tx_size).saturating_sub(1)
}

pub fn transaction_is_fully_signed(transaction: &Transaction) -> bool {
    let expected_signature_count = transaction.message.header.num_required_signatures as usize;
    let actual_signature_count = transaction.signatures.len();
    expected_signature_count == actual_signature_count && transaction.is_signed()
}

pub enum ProgramArtifacts {
    Native(NativeProgramArtifacts),
    Anchor(AnchorProgramArtifacts),
}

impl ProgramArtifacts {
    pub fn from_value(value: &Value) -> Result<Self, Diagnostic> {
        let map =
            value.as_object().ok_or(diagnosed_error!("program artifacts must be an object"))?;

        let framework = map
            .get("framework")
            .ok_or(diagnosed_error!("program artifacts must have a 'framework' field"))?
            .as_string()
            .ok_or(diagnosed_error!("'framework' field must be a string"))?;

        match framework {
            "native" => {
                let artifacts = NativeProgramArtifacts::from_value(value)?;
                Ok(ProgramArtifacts::Native(artifacts))
            }
            "anchor" => {
                let artifacts = AnchorProgramArtifacts::from_map(map)?;
                Ok(ProgramArtifacts::Anchor(artifacts))
            }
            _ => Err(diagnosed_error!("unsupported framework: {framework}")),
        }
    }
    pub fn program_id(&self) -> Pubkey {
        match self {
            ProgramArtifacts::Native(artifacts) => artifacts.program_id,
            ProgramArtifacts::Anchor(artifacts) => artifacts.program_id,
        }
    }
    pub fn keypair(&self) -> Option<Result<Keypair, Diagnostic>> {
        self.keypair_bytes().map(|bytes| {
            Keypair::try_from(bytes.as_ref())
                .map_err(|e| diagnosed_error!("failed to deserialize keypair: {e}"))
        })
    }

    pub fn keypair_bytes(&self) -> Option<Vec<u8>> {
        match self {
            ProgramArtifacts::Native(artifacts) => {
                artifacts.keypair.as_ref().map(|k| k.to_bytes().to_vec())
            }
            ProgramArtifacts::Anchor(artifacts) => {
                artifacts.keypair.as_ref().map(|k| k.to_bytes().to_vec())
            }
        }
    }
    pub fn bin(&self) -> &Vec<u8> {
        match self {
            ProgramArtifacts::Native(artifacts) => &artifacts.bin,
            ProgramArtifacts::Anchor(artifacts) => &artifacts.bin,
        }
    }
    pub fn idl(&self) -> Result<Option<String>, Diagnostic> {
        match self {
            ProgramArtifacts::Native(_) => Ok(None),
            ProgramArtifacts::Anchor(artifacts) => Some(
                serde_json::to_string(&artifacts.idl)
                    .map_err(|e| diagnosed_error!("invalid anchor idl: {e}")),
            )
            .transpose(),
        }
    }
}

pub fn validate_program_so(bin: &[u8]) -> Result<(), Diagnostic> {
    if bin.len() < 4 || &bin[0..4] != b"\x7fELF" {
        return Err(diagnosed_error!("program binary is not a valid ELF file"));
    }
    Ok(())
}
