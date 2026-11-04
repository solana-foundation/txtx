use std::path::PathBuf;

use crate::runbook::{
    RunbookExecutionSnapshot, RunbookTopLevelInputsMap, DEFAULT_TOP_LEVEL_INPUTS_NAME,
};
use txtx_addon_kit::helpers::fs::{FileAccessor, FileLocation};
use txtx_addon_kit::indexmap::IndexMap;
use txtx_addon_kit::serde::{Deserialize, Serialize};

pub mod file;

pub use file::WorkspaceManifestFile;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub name: String,
    pub id: String,
    pub runbooks: Vec<RunbookMetadata>,
    pub environments: IndexMap<String, IndexMap<String, String>>,
    #[serde(skip_serializing, skip_deserializing)]
    pub location: Option<FileLocation>,
}

impl WorkspaceManifest {
    pub fn new(name: String) -> Self {
        let id = normalize_user_input(&name);
        WorkspaceManifest {
            name,
            id,
            runbooks: vec![],
            environments: IndexMap::new(),
            location: None,
        }
    }

    pub async fn from_file_accessor(
        location: &FileLocation,
        file_accessor: &dyn FileAccessor,
    ) -> Result<WorkspaceManifest, String> {
        let content = file_accessor.read_file(location.to_string()).await?;

        let manifest_file = WorkspaceManifestFile::from_yaml_slice(content.as_bytes())?;
        WorkspaceManifest::from_manifest_file(manifest_file, location)
    }

    pub fn from_location(location: &FileLocation) -> Result<WorkspaceManifest, String> {
        let manifest_file_content = location.read_content()?;
        let manifest_file = WorkspaceManifestFile::from_yaml_slice(&manifest_file_content[..])?;

        WorkspaceManifest::from_manifest_file(manifest_file, location)
    }

    pub fn get_runbook_metadata_from_location(
        &self,
        runbook_location: &str,
    ) -> Option<&RunbookMetadata> {
        for r in self.runbooks.iter() {
            if r.location.eq(runbook_location) {
                return Some(r);
            }
        }
        None
    }

    pub fn from_manifest_file(
        manifest_file: WorkspaceManifestFile,
        manifest_location: &FileLocation,
    ) -> Result<WorkspaceManifest, String> {
        let manifest = WorkspaceManifest {
            name: manifest_file.name,
            id: manifest_file.id,
            runbooks: manifest_file
                .runbooks
                .iter()
                .map(|e| RunbookMetadata {
                    location: e.location.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    state: e
                        .state
                        .as_ref()
                        .map(|s| {
                            s.location.clone().map(|f| {
                                let mut location = manifest_location.clone();
                                location = location
                                    .get_parent_location()
                                    .expect("unable to create state destination path");
                                location
                                    .append_path(&f)
                                    .expect("unable to create state destination path");
                                RunbookStateLocation::Location(location)
                            })
                        })
                        .unwrap_or(None),
                })
                .collect::<Vec<_>>(),
            environments: manifest_file.environments.clone(),
            location: Some(manifest_location.clone()),
        };
        Ok(manifest)
    }

    pub fn get_runbook_inputs(
        &self,
        selector: &Option<String>,
        cli_inputs: &Vec<String>,
        buffer_stdin: Option<String>,
    ) -> Result<RunbookTopLevelInputsMap, String> {
        if let Some(selector) = selector {
            if self.environments.get(selector).is_none() {
                return Err(format!("environment '{}' unknown from manifest", selector));
            }
        }

        let mut inputs_map =
            RunbookTopLevelInputsMap::from_environment_map(selector, &self.environments);

        inputs_map.override_values_with_cli_inputs(cli_inputs, buffer_stdin)?;
        Ok(inputs_map)
    }
}

fn normalize_user_input(input: &str) -> String {
    let normalized = input.to_lowercase().replace(" ", "-");
    // only allow alphanumeric
    let slug = normalized.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>();
    slug
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RunbookStateLocation {
    Location(FileLocation),
}

impl RunbookStateLocation {
    pub fn get_location_for_ctx(
        &self,
        runbook_id: &str,
        environment: Option<&str>,
    ) -> FileLocation {
        match &self {
            RunbookStateLocation::Location(location) => {
                let mut location = location.clone();
                location
                    .append_path(&format!(
                        "{}.{}.tx-state.json",
                        runbook_id,
                        environment.unwrap_or(&DEFAULT_TOP_LEVEL_INPUTS_NAME.to_ascii_lowercase())
                    ))
                    .expect("unable to create state destination path");
                location
            }
        }
    }

    pub fn load_execution_snapshot(
        &self,
        load_lock_file_if_exists: bool,
        runbook_id: &str,
        environment_selector: &str,
    ) -> Result<RunbookExecutionSnapshot, String> {
        let state_file_location = self.get_location_for_ctx(runbook_id, Some(environment_selector));
        let file_to_load = if load_lock_file_if_exists {
            match RunbookTransientStateLocation::from_state_file_location(&state_file_location) {
                Some(RunbookTransientStateLocation(lockfile_location)) => lockfile_location,
                None => state_file_location.clone(),
            }
        } else {
            state_file_location.clone()
        };

        let snapshot_bytes = file_to_load.read_content()?;
        if snapshot_bytes.is_empty() {
            return Err(format!("unable to read {}: file empty", file_to_load));
        }
        let snapshot: RunbookExecutionSnapshot = serde_json::from_slice(&snapshot_bytes)
            .map_err(|e| format!("unable to read {}: {}", file_to_load, e.to_string()))?;
        Ok(snapshot)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunbookTransientStateLocation(pub FileLocation);

impl RunbookTransientStateLocation {
    pub fn get_location_from_state_file_location(
        state_file_location: &FileLocation,
    ) -> FileLocation {
        let lock_file_name = format!("{}.lock", state_file_location.get_file_name().unwrap());
        let mut lock_file_location = state_file_location.get_parent_location().unwrap();
        lock_file_location.append_path(&lock_file_name).unwrap();
        lock_file_location
    }

    pub fn from_state_file_location(state_file_location: &FileLocation) -> Option<Self> {
        let lock_file_location =
            RunbookTransientStateLocation::get_location_from_state_file_location(
                state_file_location,
            );
        if lock_file_location.exists() {
            Some(Self(lock_file_location))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunbookMetadata {
    pub location: String,
    pub description: Option<String>,
    pub name: String,
    pub state: Option<RunbookStateLocation>,
}

impl RunbookMetadata {
    pub fn new(action: &str, name: &str, description: Option<String>) -> Self {
        let mut location = PathBuf::new();
        location.push("runbooks");
        location.push(action);
        location.push(format!("{}.tx", name));
        let location = location.display().to_string();
        RunbookMetadata { location, description, name: name.to_string(), state: None }
    }
}
