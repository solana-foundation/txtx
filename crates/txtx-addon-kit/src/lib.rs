#[macro_use]
extern crate serde_derive;

#[macro_use]
extern crate lazy_static;

#[macro_use]
mod macros;
pub mod constants;

pub use hex;
pub use indoc::formatdoc;
pub use indoc::indoc;
use types::commands::CommandInputsEvaluationResult;
use types::commands::CommandInstance;
use types::diagnostics::Diagnostic;
use types::AddonPostProcessingResult;
use types::ConstructDid;
pub use uuid;
pub extern crate crossbeam_channel as channel;
pub use futures;
pub use hmac;
pub use indexmap;
pub use libsecp256k1 as secp256k1;
pub use pbkdf2;

pub use hcl_edit as hcl;
use std::{collections::HashMap, fmt::Debug};
use types::{
    commands::{CommandId, PreCommandSpecification},
    functions::FunctionSpecification,
    signers::SignerSpecification,
};

pub use keccak_hash;
pub use reqwest;
pub use serde;
pub use serde_json;
pub use sha2;

use crate::types::types::Value;

pub mod crypto;
pub mod helpers;
pub mod types;

lazy_static! {
    pub static ref DEFAULT_ADDON_DOCUMENTATION_TEMPLATE: String =
        include_str!("doc/default_addon_template.mdx").to_string();
    pub static ref DEFAULT_ADDON_FUNCTIONS_TEMPLATE: String =
        include_str!("doc/default_addon_functions_template.mdx").to_string();
    pub static ref DEFAULT_ADDON_ACTIONS_TEMPLATE: String =
        include_str!("doc/default_addon_actions_template.mdx").to_string();
    pub static ref DEFAULT_ADDON_WALLETS_TEMPLATE: String =
        include_str!("doc/default_addon_signers_template.mdx").to_string();
    pub static ref DEFAULT_ADDON_OVERVIEW_TEMPLATE: String =
        include_str!("doc/default_addon_overview_template.mdx").to_string();
}

///
pub trait Addon: Debug + Sync + Send {
    ///
    fn get_name(self: &Self) -> &str;
    ///
    fn get_description(self: &Self) -> &str;
    ///
    fn get_namespace(self: &Self) -> &str;
    ///
    fn get_functions(self: &Self) -> Vec<FunctionSpecification> {
        vec![]
    }
    ///
    fn get_actions(&self) -> Vec<PreCommandSpecification> {
        vec![]
    }
    ///
    fn get_signers(&self) -> Vec<SignerSpecification> {
        vec![]
    }
    fn to_json(&self, _value: &Value) -> Result<Option<serde_json::Value>, Diagnostic> {
        Ok(None)
    }
    ///
    fn build_function_lookup(self: &Self) -> HashMap<String, FunctionSpecification> {
        let mut functions = HashMap::new();
        for function in self.get_functions().into_iter() {
            functions.insert(function.name.clone(), function);
        }
        functions
    }
    ///
    fn build_command_lookup(self: &Self) -> HashMap<CommandId, PreCommandSpecification> {
        let mut commands = HashMap::new();

        for command in self.get_actions().into_iter() {
            let matcher = match &command {
                PreCommandSpecification::Atomic(command) => command.matcher.clone(),
                PreCommandSpecification::Composite(command) => command.matcher.clone(),
            };
            commands.insert(CommandId::Action(matcher), command);
        }
        commands
    }
    ///
    fn build_signer_lookup(self: &Self) -> HashMap<String, SignerSpecification> {
        let mut signer_specs = HashMap::new();

        for signer in self.get_signers().into_iter() {
            signer_specs.insert(signer.matcher.clone(), signer);
        }

        signer_specs
    }
    ///
    fn get_domain_specific_commands_inputs_dependencies<'a>(
        self: &Self,
        _commands_instances: &'a Vec<(
            ConstructDid,
            &'a CommandInstance,
            Option<&'a CommandInputsEvaluationResult>,
        )>,
    ) -> Result<AddonPostProcessingResult, (Diagnostic, ConstructDid)> {
        Ok(AddonPostProcessingResult::new())
    }
}
