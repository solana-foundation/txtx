use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_request::RpcRequest;
use solana_pubkey::Pubkey;

use txtx_addon_kit::{
    indexmap::IndexMap,
    types::{
        diagnostics::Diagnostic, frontend::LogDispatcher, stores::ValueStore, types::Value,
        AuthorizationContext,
    },
};
use txtx_addon_network_svm_types::{anchor::types::Idl, SvmValue};

use crate::{
    codec::{utils::cheatcode_deploy_program, validate_program_so},
    constants::DEPLOY_PROGRAM,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfpoolDeployProgram {
    #[serde(skip)]
    pub program_id: Pubkey,
    #[serde(skip)]
    pub binary: Vec<u8>,
    #[serde(skip)]
    pub authority: Option<Pubkey>,
    #[serde(skip)]
    pub idl: Option<Idl>,
}

impl SurfpoolDeployProgram {
    pub fn from_map(
        map: &mut IndexMap<String, Value>,
        auth_ctx: &AuthorizationContext,
    ) -> Result<Self, Diagnostic> {
        let some_program_id = map.swap_remove("program_id");
        let program_id = some_program_id
            .map(|p| {
                SvmValue::to_pubkey(&p)
                    .map_err(|e| diagnosed_error!("invalid 'program_id' field: {e}"))
            })
            .ok_or_else(|| diagnosed_error!("missing required 'program_id'"))??;

        let binary = {
            let some_binary_path = map.swap_remove("binary_path");
            let binary_path = some_binary_path
                .ok_or_else(|| diagnosed_error!("missing required 'binary_path'"))?;
            let binary_path = binary_path
                .as_string()
                .ok_or(diagnosed_error!("'binary_path' must be a string"))?;

            let binary_path = auth_ctx
                .get_file_location_from_path_buf(&PathBuf::from(binary_path))
                .map_err(|e| diagnosed_error!("failed to get program binary path: {e}"))?;

            let bin = binary_path.read_content().map_err(|e| {
                diagnosed_error!(
                    "invalid program binary location {}: {}",
                    &binary_path.to_string(),
                    e
                )
            })?;

            validate_program_so(&bin)?;
            bin
        };

        let idl = {
            let some_idl_path = map.swap_remove("idl_path");
            if let Some(idl_path) = some_idl_path {
                let idl_path =
                    idl_path.as_string().ok_or(diagnosed_error!("'idl_path' must be a string"))?;

                let idl_path =
                    auth_ctx
                        .get_file_location_from_path_buf(&PathBuf::from(idl_path))
                        .map_err(|e| diagnosed_error!("failed to get program idl path: {e}"))?;

                if !idl_path.exists() {
                    return Err(diagnosed_error!(
                        "provided idl path does not exist: {}",
                        &idl_path.to_string()
                    ));
                }

                let idl_str = idl_path.read_content_as_utf8().map_err(|e| {
                    diagnosed_error!("invalid idl location {}: {}", &idl_path.to_string(), e)
                })?;

                let idl = crate::codec::idl::IdlRef::from_str(&idl_str).map_err(|e| {
                    diagnosed_error!("invalid idl at location {}: {}", &idl_path.to_string(), e)
                })?;
                Some(idl.idl)
            } else {
                None
            }
        };

        let some_authority = map.swap_remove("authority");
        let authority = some_authority
            .map(|p| {
                SvmValue::to_pubkey(&p)
                    .map_err(|e| diagnosed_error!("invalid 'authority' field: {e}"))
            })
            .transpose()?;

        Ok(Self { program_id, binary, authority, idl })
    }

    pub fn parse_value_store(
        values: &ValueStore,
        auth_ctx: &AuthorizationContext,
    ) -> Result<Vec<Self>, Diagnostic> {
        let mut deploy_programs = vec![];

        let deploy_program_data = values
            .get_value(DEPLOY_PROGRAM)
            .map(|v| {
                v.as_map().ok_or_else(|| diagnosed_error!("'deploy_program' must be a map type"))
            })
            .transpose()?;

        let Some(deploy_program_data) = deploy_program_data else {
            return Ok(vec![]);
        };

        let mut deploy_program_data = deploy_program_data
            .iter()
            .map(|i| {
                i.as_object()
                    .map(|o| o.clone())
                    .ok_or(diagnosed_error!("'deploy_program' must be a map type"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (i, deploy_program) in deploy_program_data.iter_mut().enumerate() {
            let prefix = format!("failed to parse `deploy_program` map #{}", i + 1);
            let account = SurfpoolDeployProgram::from_map(deploy_program, auth_ctx)
                .map_err(|e| diagnosed_error!("{prefix}: {e}"))?;

            deploy_programs.push(account);
        }

        Ok(deploy_programs)
    }

    fn update_status(&self, logger: &LogDispatcher, index: usize, total: usize) {
        logger.success_info(
            format!("Deployment #{}/{} Complete", index + 1, total),
            format!("Program {} Deployed", self.program_id.to_string()),
        );
    }

    pub async fn process_updates(
        program_deployments: Vec<Self>,
        rpc_client: &RpcClient,
        logger: &LogDispatcher,
    ) -> Result<(), Diagnostic> {
        let len = program_deployments.len();
        for (i, program_deployment) in program_deployments.into_iter().enumerate() {
            cheatcode_deploy_program(
                rpc_client,
                program_deployment.program_id,
                &program_deployment.binary,
                program_deployment.authority,
                logger,
            )
            .await?;
            if let Some(idl) = &program_deployment.idl {
                rpc_client
                    .send::<serde_json::Value>(
                        RpcRequest::Custom { method: "surfnet_registerIdl" },
                        json!([serde_json::to_string(idl).map_err(|e| {
                            diagnosed_error!("failed to serialize idl for rpc call: {e}")
                        })?]),
                    )
                    .await
                    .map_err(|e| diagnosed_error!("failed to register idl via rpc call: {e}"))?;
            }
            program_deployment.update_status(logger, i, len);
        }
        Ok(())
    }
}
