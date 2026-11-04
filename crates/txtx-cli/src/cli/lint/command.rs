//! Command interface for the linter
//!
//! This module provides the CLI command handling for the lint command.

use std::fmt;
use std::path::PathBuf;
use txtx_core::manifest::WorkspaceManifest;
use super::{LinterConfig, Linter, LinterError, Format as LinterFormat, WorkspaceAnalyzer};

/// Represents a CLI command template to execute a runbook
struct CliTemplate {
    runbook_name: String,
    manifest_path: Option<String>,
    environment: Option<String>,
    variables: Vec<txtx_core::runbook::variables::RunbookVariable>,
}

impl fmt::Display for CliTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const CMD_LINEBREAK: &str = " \\\n ";

        write!(f, "txtx run {}", self.runbook_name)?;

        if let Some(ref manifest) = self.manifest_path {
            write!(f, "{CMD_LINEBREAK} --manifest-file-path {}", manifest)?;
        }

        if let Some(ref env) = self.environment {
            write!(f, "{CMD_LINEBREAK} --env {}", env)?;
        }

        let mut variables = self.variables.clone();
        variables.sort_by(|a, b| a.name.cmp(&b.name));

        for var in &variables {
            let value = var.resolved_value.as_ref()
                .cloned()
                .unwrap_or_else(|| format!("\"${}\"", var.name.to_uppercase().replace('-', "_")));
            write!(f, "{CMD_LINEBREAK} --input {}={}", var.name, value)?;
        }

        Ok(())
    }
}

/// Options for running the linter
#[derive(Debug, Clone)]
pub struct LinterOptions {
    /// Path to custom linter configuration file
    pub config_path: Option<String>,
    /// Initialize a new linter configuration file
    pub init: bool,
}

// Future features (not yet implemented):
// - disabled_rules: Vec<String> - Disable specific rules
// - only_rules: Vec<String> - Run only specific rules
// - fix: bool - Automatically fix issues where possible

/// Main entry point for the lint command
pub fn run_lint(
    runbook_path: Option<String>,
    manifest_path: Option<String>,
    environment: Option<String>,
    cli_inputs: Vec<(String, String)>,
    format: LinterFormat,
    linter_options: LinterOptions,
    gen_cli: bool,
    gen_cli_full: bool,
) -> Result<(), LinterError> {
    // Handle --init flag
    if linter_options.init {
        return init_linter_config();
    }

    // Handle --gen-cli and --gen-cli-full
    if gen_cli || gen_cli_full {
        return handle_gen_cli(
            runbook_path.as_deref(),
            manifest_path.as_deref(),
            environment.as_deref(),
            &cli_inputs,
            gen_cli_full,
        );
    }

    // Create linter configuration with config file support
    let config = LinterConfig::with_config_file(
        manifest_path.map(PathBuf::from),
        runbook_path.clone(),
        environment,
        cli_inputs,
        format,
        linter_options.config_path.as_deref(),
    );

    // Run the linter
    let linter = Linter::new(&config)?;

    match runbook_path {
        Some(ref name) => linter.lint_runbook(name),
        None => linter.lint_all(),
    }
}

/// Initialize a new linter configuration file
fn init_linter_config() -> Result<(), LinterError> {
    use std::fs;
    use crate::cli::lint::config::{ConfigFile, RuleConfig};
    use crate::cli::lint::rules::Severity;
    use crate::cli::lint::rule_id::CliRuleId;
    use txtx_core::validation::CoreRuleId;

    let config_path = PathBuf::from(".txtxlint.yml");

    if config_path.exists() {
        return Err(LinterError::ConfigExists(config_path));
    }

    // Create a default config using the enums
    let mut config = ConfigFile {
        extends: Some("txtx:recommended".to_string()),
        rules: std::collections::HashMap::new(),
        ignore: vec!["examples/**".to_string(), "tests/**".to_string()],
    };

    // Add custom rule overrides (these override the recommended settings)
    // Core rules
    config.rules.insert(
        CoreRuleId::UndefinedInput.as_ref().to_string(),
        RuleConfig::Severity(Severity::Error),
    );

    // CLI rules with custom settings
    config.rules.insert(
        CliRuleId::CliInputOverride.as_ref().to_string(),
        RuleConfig::Severity(Severity::Info),
    );

    config.rules.insert(
        CliRuleId::InputNamingConvention.as_ref().to_string(),
        RuleConfig::Full {
            severity: Severity::Warning,
            options: {
                let mut opts = std::collections::HashMap::new();
                opts.insert("convention".to_string(), serde_json::Value::String("SCREAMING_SNAKE_CASE".to_string()));
                opts
            },
        },
    );

    config.rules.insert(
        CliRuleId::NoSensitiveData.as_ref().to_string(),
        RuleConfig::Severity(Severity::Warning),
    );

    // Serialize to YAML with comments
    let yaml_content = serde_saphyr::to_string(&config)
        .map_err(|e| LinterError::Other(format!("Failed to serialize config: {}", e)))?;

    // Add header comment
    let full_content = format!(
        "# Txtx Linter Configuration (Experimental)\n\
         # WARNING: This configuration format is experimental and may change in future versions.\n\
         # https://docs.txtx.io/linter\n\n{}",
        yaml_content
    );

    fs::write(&config_path, full_content)?;

    println!("Created .txtxlint.yml with recommended settings");
    Ok(())
}

/// Handle --gen-cli and --gen-cli-full functionality
fn handle_gen_cli(
    runbook_path: Option<&str>,
    manifest_path: Option<&str>,
    environment: Option<&str>,
    cli_inputs: &[(String, String)],
    include_all: bool,
) -> Result<(), LinterError> {
    use txtx_core::runbook::variables::RunbookVariableIterator;
    use txtx_addon_kit::helpers::fs::FileLocation;
    use txtx_core::manifest::file::read_runbook_from_location;
    use crate::cli::common::addon_registry;

    let runbook_path = runbook_path.ok_or_else(|| LinterError::Other("Runbook path required for --gen-cli".to_string()))?;
    let path = PathBuf::from(runbook_path);

    // Try to determine the runbook name and location
    let (runbook_name, _file_location, runbook_sources) = if path.exists() && path.extension().map_or(false, |ext| ext == "tx") {
        // Direct file path
        let file_location = FileLocation::from_path(path.clone());
        let (_, _, runbook_sources) = read_runbook_from_location(
            &file_location,
            &None,
            &environment.map(|s| s.to_string()),
            None,
        ).map_err(|e| LinterError::RunbookResolution(e))?;
        let name = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("runbook")
            .to_string();
        (name, file_location, runbook_sources)
    } else {
        // Resolve runbook from manifest
        let manifest_path = manifest_path
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./txtx.yml"));

        let _manifest = load_manifest(&manifest_path)?;

        // Create workspace analyzer with the appropriate configuration
        let config = LinterConfig::new(
            Some(manifest_path),
            None,
            environment.map(String::from),
            vec![],
            LinterFormat::Json,
        );
        let workspace = WorkspaceAnalyzer::new(&config)?;

        // Resolve runbook sources from the manifest
        let runbook_sources = workspace.resolve_runbook_sources(runbook_path)?;

        // Use runbook path as the display name
        let name = runbook_path.to_string();

        // Create a placeholder file location - actual resolution is handled by workspace analyzer
        let file_location = FileLocation::from_path(PathBuf::from(runbook_path));
        (name, file_location, runbook_sources)
    };

    // Load or create manifest
    let manifest = if let Some(manifest_path) = manifest_path {
        load_manifest(&PathBuf::from(manifest_path))?
    } else {
        match load_manifest(&PathBuf::from("./txtx.yml")) {
            Ok(m) => m,
            Err(_) => WorkspaceManifest::new("temp".to_string())
        }
    };

    // Get addon specs
    let addons = addon_registry::get_all_addons();
    let addon_specs = addon_registry::extract_addon_specifications(&addons);

    // Create iterator
    let iterator = RunbookVariableIterator::new_with_cli_inputs(
        &runbook_sources,
        &manifest,
        environment,
        addon_specs,
        cli_inputs,
    ).map_err(|e| LinterError::Other(e))?;

    // Collect variables
    let variables: Vec<_> = if include_all {
        iterator.collect()
    } else {
        iterator.undefined_or_cli_provided().collect()
    };

    let template = CliTemplate {
        runbook_name,
        manifest_path: manifest_path.map(String::from),
        environment: environment.map(String::from),
        variables,
    };

    println!("{template}");
    Ok(())
}

/// Load workspace manifest
fn load_manifest(path: &PathBuf) -> Result<WorkspaceManifest, LinterError> {
    let path_str = path.to_str()
        .ok_or_else(|| LinterError::InvalidConfig("Invalid manifest path".to_string()))?;

    crate::cli::runbooks::load_workspace_manifest_from_manifest_path(path_str)
        .map_err(|e| LinterError::ManifestLoad {
            path: path.clone(),
            message: e,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use txtx_core::runbook::variables::{RunbookVariable, VariableSource};

    fn make_variable(name: &str, resolved_value: Option<&str>) -> RunbookVariable {
        RunbookVariable {
            name: name.to_string(),
            full_path: format!("input.{}", name),
            resolved_value: resolved_value.map(String::from),
            source: VariableSource::Undefined,
            references: vec![],
        }
    }

    #[test]
    fn test_cli_template_basic() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: None,
            environment: None,
            variables: vec![],
        };
        assert_eq!(template.to_string(), "txtx run deploy");
    }

    #[test]
    fn test_cli_template_with_manifest() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: Some("./txtx.yml".to_string()),
            environment: None,
            variables: vec![],
        };
        let output = template.to_string();
        assert!(output.contains("txtx run deploy"));
        assert!(output.contains("--manifest-file-path ./txtx.yml"));
    }

    #[test]
    fn test_cli_template_with_environment() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: None,
            environment: Some("production".to_string()),
            variables: vec![],
        };
        let output = template.to_string();
        assert!(output.contains("txtx run deploy"));
        assert!(output.contains("--env production"));
    }

    #[test]
    fn test_cli_template_with_variables() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: None,
            environment: None,
            variables: vec![
                make_variable("api_key", None),
                make_variable("region", Some("us-west-1")),
            ],
        };
        let output = template.to_string();
        assert!(output.contains("--input api_key=\"$API_KEY\""));
        assert!(output.contains("--input region=us-west-1"));
    }

    #[test]
    fn test_cli_template_variables_sorted_alphabetically() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: None,
            environment: None,
            variables: vec![
                make_variable("zebra", None),
                make_variable("alpha", None),
                make_variable("middle", None),
            ],
        };
        let output = template.to_string();
        let alpha_pos = output.find("alpha").unwrap();
        let middle_pos = output.find("middle").unwrap();
        let zebra_pos = output.find("zebra").unwrap();
        assert!(alpha_pos < middle_pos);
        assert!(middle_pos < zebra_pos);
    }

    #[test]
    fn test_cli_template_variable_name_with_dashes() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: None,
            environment: None,
            variables: vec![make_variable("my-api-key", None)],
        };
        let output = template.to_string();
        // Dashes in variable name should become underscores in env var placeholder
        assert!(output.contains("--input my-api-key=\"$MY_API_KEY\""));
    }

    #[test]
    fn test_cli_template_full() {
        let template = CliTemplate {
            runbook_name: "deploy".to_string(),
            manifest_path: Some("./custom.yml".to_string()),
            environment: Some("staging".to_string()),
            variables: vec![
                make_variable("token", Some("secret123")),
                make_variable("count", None),
            ],
        };
        let output = template.to_string();
        assert!(output.contains("txtx run deploy"));
        assert!(output.contains("--manifest-file-path ./custom.yml"));
        assert!(output.contains("--env staging"));
        assert!(output.contains("--input count=\"$COUNT\""));
        assert!(output.contains("--input token=secret123"));
    }

    #[test]
    fn test_lint_handles_none_manifest_path() {
        let linter_options = LinterOptions {
            config_path: None,
            init: false,
        };

        // When manifest_path is None and the runbook is not a direct file path,
        // the function should try to load from default manifest
        let result = run_lint(
            Some("test-runbook".to_string()),
            None, // This should default to "./txtx.yml"
            None, // No environment specified
            vec![],
            LinterFormat::Json,
            linter_options,
            false,
            false,
        );

        // The function should fail because the manifest doesn't exist in test environment
        // but it should fail gracefully, not panic
        assert!(result.is_err());
        // Just verify it returns an error (any LinterError variant is acceptable)
        result.unwrap_err();
    }

    #[test]
    fn test_lint_all_runbooks_defaults_manifest_path() {
        let linter_options = LinterOptions {
            config_path: None,
            init: false,
        };

        // When manifest_path is None, it should default to "./txtx.yml"
        let result = run_lint(
            None, // Lint all runbooks
            None, // This should default to "./txtx.yml"
            None, // No environment specified
            vec![],
            LinterFormat::Json,
            linter_options,
            false,
            false,
        );

        // Should attempt to load default manifest and fail gracefully
        // Either returns Ok(()) if no runbooks found, or error if manifest invalid
        // but should not panic
        match result {
            Ok(_) => {
                // No runbooks found is okay
            }
            Err(_) => {
                // Should return a LinterError, not panic
                // The specific error type doesn't matter for this test
            }
        }
    }
}
