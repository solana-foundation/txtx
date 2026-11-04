use serde::{Deserialize, Deserializer, Serialize};
use txtx_addon_kit::{
    helpers::fs::{get_txtx_files_paths, FileLocation},
    indexmap::IndexMap,
    types::RunbookId,
};

use crate::runbook::{Runbook, RunbookSources};

use super::{RunbookStateLocation, WorkspaceManifest};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceManifestFile {
    pub name: String,
    pub id: String,
    #[serde(deserialize_with = "null_as_default")]
    pub runbooks: Vec<RunbookMetadataFile>,
    #[serde(deserialize_with = "deserialize_environments")]
    pub environments: IndexMap<String, IndexMap<String, String>>,
}

impl WorkspaceManifestFile {
    pub fn new(name: String) -> Self {
        let id = normalize_user_input(&name);
        WorkspaceManifestFile { name, id, runbooks: vec![], environments: IndexMap::new() }
    }

    /// Parses the contents of a `txtx.yml` manifest.
    ///
    /// Only YAML 1.2 booleans are recognized, so values such as `on`, `off`, `yes`
    /// and `no` are read as strings. Blank `runbooks:`, `environments:` and
    /// per-environment entries are read as empty, and a blank environment value is
    /// read as an empty string.
    pub fn from_yaml_slice(content: &[u8]) -> Result<Self, String> {
        let options = serde_saphyr::options! { strict_booleans: true };
        serde_saphyr::from_slice_with_options(content, options)
            .map_err(|e| format!("txtx.yml file malformatted {:?}", e))
    }
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

// The `txtx new` template renders blank YAML values for workspaces without
// environments or environment values, which YAML parses as null.
fn deserialize_environments<'de, D>(
    deserializer: D,
) -> Result<IndexMap<String, IndexMap<String, String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let environments: Option<IndexMap<String, Option<IndexMap<String, Option<String>>>>> =
        Option::deserialize(deserializer)?;
    Ok(environments
        .unwrap_or_default()
        .into_iter()
        .map(|(name, values)| {
            let values = values
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| (key, value.unwrap_or_default()))
                .collect();
            (name, values)
        })
        .collect())
}

fn normalize_user_input(input: &str) -> String {
    let normalized = input.to_lowercase().replace(" ", "-");
    // only allow alphanumeric
    let slug = normalized.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>();
    slug
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunbookMetadataFile {
    pub location: String,
    pub name: String,
    pub description: Option<String>,
    pub state: Option<RunbookStateFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunbookStateFile {
    pub location: Option<String>,
}

pub fn read_runbooks_from_manifest(
    manifest: &WorkspaceManifest,
    environment_selector: &Option<String>,
    runbooks_filter_in: Option<&Vec<String>>,
) -> Result<IndexMap<String, (Runbook, RunbookSources, String, Option<RunbookStateLocation>)>, String>
{
    let mut runbooks = IndexMap::new();

    let root_path =
        manifest.location.as_ref().expect("unable to get location").get_parent_location()?;

    for runbook_metadata in manifest.runbooks.iter() {
        if let Some(runbooks_filter_in) = runbooks_filter_in {
            if !runbooks_filter_in.contains(&runbook_metadata.name) {
                continue;
            }
        }

        let mut package_location = root_path.clone();
        package_location.append_path(&runbook_metadata.location)?;
        if let Ok((_, runbook, sources)) = read_runbook_from_location(
            &package_location,
            &runbook_metadata.description,
            environment_selector,
            Some(&runbook_metadata.name),
        ) {
            runbooks.insert(
                runbook_metadata.name.to_string(),
                (
                    runbook,
                    sources,
                    runbook_metadata.name.to_string(),
                    runbook_metadata.state.clone(),
                ),
            );
        }
    }
    Ok(runbooks)
}

pub fn read_runbook_from_location(
    location: &FileLocation,
    description: &Option<String>,
    environment_selector: &Option<String>,
    runbook_id: Option<&str>,
) -> Result<(String, Runbook, RunbookSources), String> {
    let runbook_name = runbook_id
        .and_then(|id| Some(id.to_string()))
        .unwrap_or(location.get_file_name().unwrap_or(location.to_string()));
    let mut runbook_sources = RunbookSources::new();
    let package_location = location.clone();
    match std::fs::read_dir(package_location.to_string()) {
        Ok(_) => {
            let files = get_txtx_files_paths(&package_location.to_string(), environment_selector)
                .map_err(|e| format!("unable to read directory: {}", e))?;
            for file_path in files.into_iter() {
                let location = FileLocation::from_path(file_path);
                let file_content = location.read_content_as_utf8()?;
                runbook_sources.add_source(runbook_name.clone(), location, file_content);
            }
        }
        Err(_) => {
            let file_content = package_location.read_content_as_utf8()?;
            runbook_sources.add_source(runbook_name.clone(), package_location, file_content);
        }
    }

    let runbook_id = RunbookId::new(None, None, &runbook_name);
    Ok((runbook_name, Runbook::new(runbook_id, description.clone()), runbook_sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    // `serde_yml` read blank YAML values as empty collections and strings; the
    // `txtx new` template and existing manifests rely on that.
    #[test]
    fn parses_manifest_with_blank_values() {
        let manifest = WorkspaceManifestFile::from_yaml_slice(
            br#"---
name: demo
id: demo
runbooks:
  - name: deploy
    description:
    location: runbooks/deploy.tx
environments:
  devnet:
    chain_id: 11155111
    verify: off
    api_key:
  mainnet:
"#,
        )
        .unwrap();

        assert_eq!(manifest.runbooks[0].description, None);
        let devnet = &manifest.environments["devnet"];
        assert_eq!(devnet["chain_id"], "11155111");
        assert_eq!(devnet["verify"], "off");
        assert_eq!(devnet["api_key"], "");
        assert!(manifest.environments["mainnet"].is_empty());

        let empty = WorkspaceManifestFile::from_yaml_slice(
            b"---\nname: demo\nid: demo\nrunbooks:\nenvironments:\n",
        )
        .unwrap();
        assert!(empty.runbooks.is_empty());
        assert!(empty.environments.is_empty());
    }
}
