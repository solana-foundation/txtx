use crate::constants::{SIGNER, SIGNERS};
use deploy_program::DEPLOY_PROGRAM;
use deploy_subraph::DEPLOY_SUBGRAPH;
use deploy_token::DEPLOY_TOKEN;
use process_instructions::PROCESS_INSTRUCTIONS;
use send_sol::SEND_SOL;
use send_token::SEND_TOKEN;
use serde::{Deserialize, Serialize};
use setup_surfnet::SETUP_SURFNET;
use solana_client::rpc_request::RpcRequest;
// use srs::create_class::CREATE_CLASS;
// use srs::create_record::CREATE_RECORD;
use txtx_addon_kit::types::commands::PreCommandSpecification;
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::{diagnostics::Diagnostic, ConstructDid, Did};

pub mod deploy_program;
pub mod deploy_subraph;
pub mod deploy_token;
pub mod process_instructions;
pub mod send_sol;
pub mod send_token;
pub mod setup_surfnet;
pub mod sign_transaction;
// pub mod srs;

fn get_signers_did(args: &ValueStore) -> Result<Vec<ConstructDid>, Diagnostic> {
    let signers = args.get_expected_array(SIGNERS)?;
    let mut res = vec![];
    for signer in signers.iter() {
        res.push(ConstructDid(Did::from_hex_string(signer.expect_string())));
    }
    Ok(res)
}
fn get_signer_did(args: &ValueStore) -> Result<ConstructDid, Diagnostic> {
    let signer = args.get_expected_string(SIGNER)?;
    Ok(ConstructDid(Did::from_hex_string(signer)))
}

pub fn get_custom_signer_did(
    args: &ValueStore,
    signer_key: &str,
) -> Result<ConstructDid, Diagnostic> {
    let signer = args.get_expected_string(signer_key)?;
    Ok(ConstructDid(Did::from_hex_string(signer)))
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RpcVersionInfo {
    /// The current version of surfnet, if RPC is a surfnet
    pub surfnet_version: Option<String>,
    /// The current version of solana-core
    pub solana_core: String,
    /// first 4 bytes of the FeatureSet identifier
    pub feature_set: Option<u32>,
}

impl RpcVersionInfo {
    pub fn fetch_blocking(
        rpc_client: &solana_client::rpc_client::RpcClient,
    ) -> Result<Self, Diagnostic> {
        rpc_client
            .send::<Self>(RpcRequest::Custom { method: "getVersion" }, serde_json::json!([]))
            .map_err(|e| diagnosed_error!("failed to fetch RPC endpoint version: {e}"))
    }
    pub async fn fetch_non_blocking(
        rpc_client: &solana_client::nonblocking::rpc_client::RpcClient,
    ) -> Result<Self, Diagnostic> {
        rpc_client
            .send::<Self>(RpcRequest::Custom { method: "getVersion" }, serde_json::json!([]))
            .await
            .map_err(|e| diagnosed_error!("failed to fetch RPC endpoint version: {e}"))
    }
}

lazy_static! {
    pub static ref ACTIONS: Vec<PreCommandSpecification> = vec![
        PROCESS_INSTRUCTIONS.clone(),
        DEPLOY_PROGRAM.clone(),
        DEPLOY_TOKEN.clone(),
        SEND_SOL.clone(),
        SEND_TOKEN.clone(),
        DEPLOY_SUBGRAPH.clone(),
        SETUP_SURFNET.clone(),
        // CREATE_CLASS.clone(),
        // CREATE_RECORD.clone(),
    ];
}
