use std::collections::HashMap;
#[cfg(feature = "dirs")]
use std::env;
use std::fmt::Display;
use std::path::PathBuf;

use commands::{PostConditionEvaluatableInput, PreConditionEvaluatableInput};
use diagnostics::Diagnostic;
use dyn_clone::DynClone;
use hcl_edit::expr::Expression;
use hcl_edit::structure::Block;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use types::{ObjectDefinition, ObjectProperty, Type};
use uuid::Uuid;

use crate::helpers::fs::FileLocation;

pub mod block_id;
pub mod cloud_interface;
pub mod commands;
pub mod construct_type;
pub mod typed_block;
pub mod diagnostic_types;
pub mod diagnostics;

// Re-export common diagnostic types for convenience
pub use diagnostic_types::{DiagnosticLevel, DiagnosticSpan, RelatedLocation};

pub mod embedded_runbooks;
pub mod frontend;
pub mod functions;
pub mod package;
pub mod signers;
pub mod stores;
pub mod types;

pub const CACHED_NONCE: &str = "cached_nonce";

#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Did(pub [u8; 32]);

impl Did {
    pub fn from_components(comps: Vec<impl AsRef<[u8]>>) -> Self {
        let mut hasher = Sha256::new();
        for comp in comps {
            hasher.update(comp);
        }
        let hash = hasher.finalize();
        Did(hash.into())
    }

    pub fn from_hex_string(source_bytes_str: &str) -> Self {
        let bytes = hex::decode(source_bytes_str).expect("invalid hex_string");
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(source_bytes: &Vec<u8>) -> Self {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&source_bytes);
        Did(bytes)
    }

    pub fn zero() -> Self {
        Self([0u8; 32])
    }

    pub fn to_string(&self) -> String {
        hex::encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn as_uuid(&self) -> Uuid {
        Uuid::from_bytes(self.0[0..16].try_into().unwrap())
    }
}

impl Serialize for Did {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{}", self))
    }
}

impl<'de> Deserialize<'de> for Did {
    fn deserialize<D>(deserializer: D) -> Result<Did, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes_hex: String = serde::Deserialize::deserialize(deserializer)?;
        let bytes = hex::decode(&bytes_hex[2..]).map_err(|e| D::Error::custom(e.to_string()))?;
        Ok(Did::from_bytes(&bytes))
    }
}

impl std::fmt::Display for Did {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl std::fmt::Debug for Did {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "0x{}", self.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunbookDid(pub Did);

impl RunbookDid {
    pub fn value(&self) -> Did {
        self.0.clone()
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RunbookId {
    /// Canonical name of the org authoring the workspace
    pub org: Option<String>,
    /// Canonical name of the workspace supporting the runbook
    pub workspace: Option<String>,
    /// Canonical name of the runbook
    pub name: String,
}

impl RunbookId {
    pub fn new(org: Option<String>, workspace: Option<String>, name: &str) -> RunbookId {
        RunbookId { org, workspace, name: name.into() }
    }
    pub fn did(&self) -> RunbookDid {
        let mut comps = vec![];
        if let Some(ref org) = self.org {
            comps.push(org.as_bytes());
        }
        if let Some(ref workspace) = self.workspace {
            comps.push(workspace.as_bytes());
        }
        comps.push(self.name.as_bytes());
        let did = Did::from_components(comps);
        RunbookDid(did)
    }

    pub fn zero() -> RunbookId {
        RunbookId { org: None, workspace: None, name: "".into() }
    }
}

pub struct RunbookInstanceContext {
    pub runbook_id: RunbookId,
    pub workspace_location: FileLocation,
    pub environment_selector: Option<String>,
}

impl RunbookInstanceContext {
    pub fn get_workspace_root(&self) -> Result<FileLocation, String> {
        self.workspace_location.get_parent_location()
    }
    pub fn environment_selector<'a>(&'a self, default: &'a str) -> &'a str {
        self.environment_selector.as_deref().unwrap_or(default)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PackageDid(pub Did);

impl PackageDid {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PackageId {
    /// Id of the Runbook
    pub runbook_id: RunbookId,
    /// Location of the package within the workspace
    pub package_location: FileLocation,
    /// Name of the package
    pub package_name: String,
}

impl PackageId {
    pub fn did(&self) -> PackageDid {
        let did = Did::from_components(vec![
            self.runbook_id.did().as_bytes(),
            self.package_name.to_string().as_bytes(),
            // todo(lgalabru): This should be done upstream.
            // Serializing is allowing us to get a canonical location.
            serde_json::json!(self.package_location).to_string().as_bytes(),
        ]);
        PackageDid(did)
    }

    pub fn zero() -> PackageId {
        PackageId {
            runbook_id: RunbookId::zero(),
            package_location: FileLocation::working_dir(),
            package_name: "".into(),
        }
    }
    pub fn from_file(
        location: &FileLocation,
        runbook_id: &RunbookId,
        package_name: &str,
    ) -> Result<Self, Diagnostic> {
        let package_location = location.get_parent_location().map_err(|e| {
            Diagnostic::error_from_string(format!("{}", e.to_string())).location(&location)
        })?;
        Ok(PackageId {
            runbook_id: runbook_id.clone(),
            package_location: package_location.clone(),
            package_name: package_name.to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub struct ConstructDid(pub Did);

impl ConstructDid {
    pub fn value(&self) -> Did {
        self.0.clone()
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn to_string(&self) -> String {
        self.0.to_string()
    }

    pub fn from_hex_string(did_str: &str) -> Self {
        ConstructDid(Did::from_hex_string(did_str))
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0.as_uuid()
    }
}

impl Display for ConstructDid {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstructId {
    /// Id of the Package
    pub package_id: PackageId,
    /// Location of the file enclosing the construct
    pub construct_location: FileLocation,
    /// Type of construct (e.g. `variable` in `variable.value``)
    pub construct_type: construct_type::ConstructType,
    /// Name of construct (e.g. `value` in `variable.value``)
    pub construct_name: String,
}

impl ConstructId {
    pub fn did(&self) -> ConstructDid {
        let did = Did::from_components(vec![
            self.package_id.did().as_bytes(),
            self.construct_type.as_ref().as_bytes(),  // Zero-cost conversion via AsRef trait
            self.construct_name.to_string().as_bytes(),
            // todo(lgalabru): This should be done upstream.
            // Serializing is allowing us to get a canonical location.
            serde_json::json!(self.construct_location).to_string().as_bytes(),
        ]);
        ConstructDid(did)
    }

    /// Get the construct type as a string reference.
    ///
    /// This is a zero-cost conversion that returns a reference to a static string.
    pub fn construct_type_str(&self) -> &str {
        self.construct_type.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct Construct {
    /// Id of the Construct
    pub construct_id: ConstructId,
}

#[derive(Debug, Clone)]
pub struct AuthorizationContext {
    pub workspace_location: FileLocation,
}

impl AuthorizationContext {
    pub fn new(workspace_location: FileLocation) -> Self {
        Self { workspace_location }
    }

    pub fn empty() -> Self {
        Self { workspace_location: FileLocation::working_dir() }
    }

    pub fn get_file_location_from_path_buf(&self, input: &PathBuf) -> Result<FileLocation, String> {
        let path_str = input.to_string_lossy();

        let loc = if let Some(stripped) = path_str.strip_prefix("~/") {
            #[cfg(feature = "dirs")]
            {
                let home = PathBuf::from(get_home_dir());
                FileLocation::from_path(home.join(stripped))
            }
            #[cfg(not(feature = "dirs"))]
            {
                let _ = stripped;
                return Err(
                    "Home directory expansion (~/) is not supported in this build".to_string(),
                );
            }
        }
        // If absolute, use as-is
        else if input.is_absolute() {
            FileLocation::from_path(input.clone())
        } else {
            let mut workspace_loc = self
                .workspace_location
                .get_parent_location()
                .map_err(|e| format!("unable to read workspace location: {e}"))?;

            workspace_loc
                .append_path(&path_str.to_string())
                .map_err(|e| format!("invalid path: {}", e))?;
            workspace_loc
        };

        Ok(loc)
    }
}

/// Gets the user's home directory, accounting for the Snap confinement environment.
/// We set out snap build to set this environment variable to the real home directory,
/// because by default, snaps run in a confined environment where the home directory is not
/// the user's actual home directory.
#[cfg(feature = "dirs")]
fn get_home_dir() -> String {
    if let Ok(real_home) = env::var("SNAP_REAL_HOME") {
        let path_buf = PathBuf::from(real_home);
        path_buf.display().to_string()
    } else {
        dirs::home_dir().unwrap().display().to_string()
    }
}

#[derive(Debug)]
pub enum ContractSourceTransform {
    FindAndReplace(String, String),
    RemapDownstreamDependencies(String, String),
}

pub struct AddonPostProcessingResult {
    pub dependencies: HashMap<ConstructDid, Vec<ConstructDid>>,
    pub transforms: HashMap<ConstructDid, Vec<ContractSourceTransform>>,
}

impl AddonPostProcessingResult {
    pub fn new() -> AddonPostProcessingResult {
        AddonPostProcessingResult { dependencies: HashMap::new(), transforms: HashMap::new() }
    }
}

#[derive(Debug, Clone)]
pub struct AddonInstance {
    pub addon_id: String,
    pub package_id: PackageId,
    pub block: Block,
}

pub trait WithEvaluatableInputs {
    fn name(&self) -> String;
    fn block(&self) -> &Block;
    fn get_expression_from_input(&self, input_name: &str) -> Option<Expression>;
    fn get_blocks_for_map(
        &self,
        input_name: &str,
        input_typing: &Type,
        input_optional: bool,
    ) -> Result<Option<Vec<Block>>, Vec<Diagnostic>>;
    fn get_expression_from_block(&self, block: &Block, prop: &ObjectProperty)
        -> Option<Expression>;
    fn get_expression_from_object(
        &self,
        input_name: &str,
        input_typing: &Type,
    ) -> Result<Option<Expression>, Vec<Diagnostic>>;
    fn get_expression_from_object_property(
        &self,
        input_name: &str,
        prop: &ObjectProperty,
    ) -> Option<Expression>;

    /// Defines the inputs for this trait type with evaluatable inputs
    fn _spec_inputs(&self) -> Vec<Box<dyn EvaluatableInput>>;

    // Merges some default inputs that are available for all commands
    // with those defined specifically for the implementer of this trait
    fn spec_inputs(&self) -> Vec<Box<dyn EvaluatableInput>> {
        let mut spec_inputs = self._spec_inputs();
        spec_inputs.push(Box::new(PreConditionEvaluatableInput::new()));
        spec_inputs
    }

    fn self_referencing_inputs(&self) -> Vec<Box<dyn EvaluatableInput>> {
        vec![Box::new(PostConditionEvaluatableInput::new())]
    }
}

pub trait EvaluatableInput: DynClone {
    fn documentation(&self) -> String;
    fn optional(&self) -> bool;
    fn typing(&self) -> &Type;
    fn name(&self) -> String;
    fn as_object(&self) -> Option<&ObjectDefinition> {
        self.typing().as_object()
    }
    fn as_array(&self) -> Option<&Box<Type>> {
        self.typing().as_array()
    }
    fn as_action(&self) -> Option<&String> {
        self.typing().as_action()
    }
    fn as_map(&self) -> Option<&ObjectDefinition> {
        self.typing().as_map()
    }
}

dyn_clone::clone_trait_object!(EvaluatableInput);
