mod cheatcode_deploy_program;
pub mod clone_program_account;
mod reset_account;
pub mod set_account;
mod set_program_authority;
mod set_token_account;
mod stream_account;
pub(crate) mod surfnet_update;
pub mod tokens;

use surfnet_update::SurfnetAccountUpdate;

use clone_program_account::SurfpoolProgramCloning;
use set_account::SurfpoolAccountUpdate;
use set_token_account::SurfpoolTokenAccountUpdate;
use solana_client::nonblocking::rpc_client::RpcClient;
use txtx_addon_kit::channel;
use txtx_addon_kit::types::cloud_interface::CloudServiceContext;
use txtx_addon_kit::types::commands::{
    return_synchronous_ok, CommandExecutionFutureResult, CommandExecutionResult,
    CommandImplementation, CommandSpecification, PreCommandSpecification,
};
use txtx_addon_kit::types::diagnostics::Diagnostic;
use txtx_addon_kit::types::frontend::{Actions, BlockEvent, LogDispatcher};
use txtx_addon_kit::types::stores::ValueStore;
use txtx_addon_kit::types::types::{RunbookSupervisionContext, Type};
use txtx_addon_kit::types::ConstructDid;
use txtx_addon_kit::uuid::Uuid;
use txtx_addon_network_svm_types::{
    CLONE_PROGRAM_ACCOUNT, DEPLOY_PROGRAM, RESET_ACCOUNT, SET_ACCOUNT_MAP, SET_PROGRAM_AUTHORITY,
    SET_TOKEN_ACCOUNT_MAP, STREAM_ACCOUNT,
};

use crate::commands::setup_surfnet::set_program_authority::SurfpoolSetProgramAuthority;
use crate::commands::RpcVersionInfo;
use crate::constants::RPC_API_URL;

lazy_static! {
    pub static ref SETUP_SURFNET: PreCommandSpecification = {
        let command = define_command! {
            SetupSurfpool => {
                name: "Setup Surfpool",
                matcher: "setup_surfnet",
                documentation: indoc!{r#"
                    `svm::setup_surfnet` can be used to configure a surfnet.
                    
                    The following operations are supported:
                     - `set_account` - used to set the lamports, owner, data, and executable fields of an account.
                     - `set_token_account` - used to set the amount, delegate, delegated amount, and close authority for a token account.
                     - `clone_program_account` - used to clone a program account from a source program id to a destination program id.
                     - `set_program_authority` - used to set the authority of a program.
                     - `deploy_program` - used to deploy a program (via a direct write to the account data rather than valid transactions) to the surfnet.
                     - `reset_account` - used to reset an account on the surfnet, removing it from the local cache to be pulled again from the upstream.
                     - `stream_account` - used to stream account data from the mainnet RPC url to the surfnet so that the local account data always matches mainnet, optionally including all owned accounts.

                "#},
                implements_signing_capability: false,
                implements_background_task_capability: true,
                inputs: [
                    description: {
                        documentation: "A description of the setup.",
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
                    set_account: {
                        documentation: "The account data to set.",
                        typing: SET_ACCOUNT_MAP.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    set_token_account: {
                        documentation: "The token account data to set.",
                        typing: SET_TOKEN_ACCOUNT_MAP.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    clone_program_account: {
                        documentation: "The program clone data to set.",
                        typing: CLONE_PROGRAM_ACCOUNT.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    set_program_authority: {
                        documentation: "The program authority data to set.",
                        typing: SET_PROGRAM_AUTHORITY.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    deploy_program: {
                        documentation: "The program deployment data to set.",
                        typing: DEPLOY_PROGRAM.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    reset_account: {
                        documentation: "The account reset data to set.",
                        typing: RESET_ACCOUNT.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    },
                    stream_account: {
                        documentation: "The account stream data to set.",
                        typing: STREAM_ACCOUNT.clone(),
                        optional: true,
                        tainting: false,
                        internal: false,
                        sensitive: false
                    }
                ],
                outputs: [],
                example: txtx_addon_kit::indoc! {r#"
                    action "setup" "svm::setup_surfnet" {
                        set_account {
                            public_key = signer.caller.public_key
                            lamports = 999999999
                        }
                        set_token_account {
                            public_key = signer.caller.public_key
                            token = "usdc"
                            amount = 1000000
                        }
                        clone_program_account {
                            source_program_id = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" // USDC program id
                            destination_program_id = variable.my_program_id
                        }
                        set_program_authority {
                            program_id = variable.my_program_id
                            authority = signer.caller.public_key
                        }
                        deploy_program {
                            program_id = variable.my_program_id
                            binary_path = "./path/to/my_program.so"
                        }
                        reset_account {
                            public_key = variable.some_pubkey
                        }
                        stream_account {
                            public_key = variable.some_pubkey
                            include_owned_accounts = true
                        }
                    }
                "#},
            }
        };

        command
    };
}

pub struct SetupSurfpool;
impl CommandImplementation for SetupSurfpool {
    fn check_instantiability(
        _ctx: &CommandSpecification,
        _args: Vec<Type>,
    ) -> Result<Type, Diagnostic> {
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

    fn run_execution(
        construct_did: &ConstructDid,
        _spec: &CommandSpecification,
        values: &ValueStore,
        progress_tx: &txtx_addon_kit::channel::Sender<BlockEvent>,
        auth_context: &txtx_addon_kit::types::AuthorizationContext,
    ) -> CommandExecutionFutureResult {
        use txtx_addon_kit::types::commands::CommandExecutionResult;

        let values = values.clone();
        let progress_tx = progress_tx.clone();
        let construct_did = construct_did.clone();
        let auth_context = auth_context.clone();

        let future = async move {
            let result = CommandExecutionResult::new();

            let rpc_api_url = values.get_expected_string(RPC_API_URL)?;

            let rpc_client = RpcClient::new(rpc_api_url.to_string());

            let version = RpcVersionInfo::fetch_non_blocking(&rpc_client).await?;
            if version.surfnet_version.is_none() {
                return Err(diagnosed_error!(
                    "RPC endpoint is not a surfnet, setup_surfnet is not supported"
                ));
            }

            let logger =
                LogDispatcher::new(construct_did.as_uuid(), "svm::setup_surfnet", &progress_tx);

            let acc_data =
                SurfpoolAccountUpdate::get_accounts_data_if_needed(&values, &rpc_client).await?;

            let account_updates = SurfpoolAccountUpdate::parse_value_store(
                &values,
                &auth_context,
                acc_data,
                &logger,
            )?;

            SurfpoolAccountUpdate::process_updates(account_updates, &rpc_client, &logger).await?;

            let token_account_updates = SurfpoolTokenAccountUpdate::parse_value_store(&values)?;
            SurfpoolTokenAccountUpdate::process_updates(
                token_account_updates,
                &rpc_client,
                &logger,
            )
            .await?;

            let program_account_clones = SurfpoolProgramCloning::parse_value_store(&values)?;
            SurfpoolProgramCloning::process_updates(program_account_clones, &rpc_client, &logger)
                .await?;

            let set_authorities = SurfpoolSetProgramAuthority::parse_value_store(&values)?;
            SurfpoolSetProgramAuthority::process_updates(set_authorities, &rpc_client, &logger)
                .await?;

            let deployments = cheatcode_deploy_program::SurfpoolDeployProgram::parse_value_store(
                &values,
                &auth_context,
            )?;
            cheatcode_deploy_program::SurfpoolDeployProgram::process_updates(
                deployments,
                &rpc_client,
                &logger,
            )
            .await?;

            let resets = reset_account::SurfpoolResetAccount::parse_value_store(&values)?;
            reset_account::SurfpoolResetAccount::process_updates(resets, &rpc_client, &logger)
                .await?;

            let streams = stream_account::SurfpoolStreamAccount::parse_value_store(&values)?;
            stream_account::SurfpoolStreamAccount::process_updates(streams, &rpc_client, &logger)
                .await?;

            Ok(result)
        };

        Ok(Box::pin(future))
    }

    fn build_background_task(
        _construct_did: &ConstructDid,
        _spec: &CommandSpecification,
        _values: &ValueStore,
        _outputs: &ValueStore,
        _progress_tx: &channel::Sender<BlockEvent>,
        _background_tasks_uuid: &Uuid,
        _supervision_context: &RunbookSupervisionContext,
        _cloud_service_context: &Option<CloudServiceContext>,
    ) -> CommandExecutionFutureResult {
        return_synchronous_ok(CommandExecutionResult::new())
    }
}
