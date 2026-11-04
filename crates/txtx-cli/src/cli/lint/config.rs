//! Linter configuration

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::fs;
use super::formatter::Format;
use super::rules::Severity;
use super::rule_id::CliRuleId;
use txtx_core::validation::CoreRuleId;
use serde::{Deserialize, Serialize};

/// Rule configuration from YAML file
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RuleConfig {
    /// Simple severity level (e.g., "error", "warning", "off")
    Severity(Severity),
    /// Full configuration with severity and options
    Full {
        severity: Severity,
        #[serde(default)]
        options: HashMap<String, serde_json::Value>,
    },
}

impl RuleConfig {
    /// Get the severity level for this rule
    pub fn severity(&self) -> Severity {
        match self {
            RuleConfig::Severity(s) => *s,
            RuleConfig::Full { severity, .. } => *severity,
        }
    }

    /// Check if rule is disabled
    pub fn is_disabled(&self) -> bool {
        self.severity() == Severity::Off
    }

    /// Get options for this rule
    #[allow(dead_code)] // Will be used when rules support custom options
    pub fn options(&self) -> Option<&HashMap<String, serde_json::Value>> {
        match self {
            RuleConfig::Severity(_) => None,
            RuleConfig::Full { options, .. } => Some(options),
        }
    }
}

/// Configuration file structure (.txtxlint.yml)
///
/// **EXPERIMENTAL**: This configuration format is experimental and may change
/// in future versions as we add support for plugins and more advanced features.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ConfigFile {
    /// Base configuration to extend (e.g., "txtx:recommended")
    #[serde(rename = "extends", skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,

    /// Rule configurations mapped by rule ID
    #[serde(default)]
    pub rules: HashMap<String, RuleConfig>,

    /// Paths to ignore during linting
    #[serde(default)]
    pub ignore: Vec<String>,
}

impl ConfigFile {
    /// Load configuration from a YAML file
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;

        // YAML 1.2 booleans only, so `off` stays a severity rather than becoming `false`.
        let options = serde_saphyr::options! { strict_booleans: true };
        serde_saphyr::from_str_with_options(&content, options)
            .map_err(|e| format!("Failed to parse YAML config: {}", e))
    }

    /// Load configuration from default locations
    pub fn load_default() -> Option<Self> {
        // Try .txtxlint.yml first, then .txtxlint.yaml
        for filename in &[".txtxlint.yml", ".txtxlint.yaml"] {
            let path = Path::new(filename);
            if path.exists() {
                if let Ok(config) = Self::from_file(path) {
                    return Some(config);
                }
            }
        }
        None
    }

    /// Load configuration from specified path or default locations
    pub fn load(config_path: Option<&str>) -> Option<Self> {
        if let Some(path) = config_path {
            Self::from_file(Path::new(path)).ok()
        } else {
            Self::load_default()
        }
    }

    /// Apply extends to get base configuration
    pub fn with_extends(&self) -> Self {
        let mut config = self.clone();

        if let Some(ref extends) = self.extends {
            if extends == "txtx:recommended" {
                // Apply recommended defaults
                let defaults = Self::recommended();

                // Merge rules (user config overrides defaults)
                for (rule, rule_config) in defaults.rules {
                    config.rules.entry(rule).or_insert(rule_config);
                }
            }
        }

        config
    }

    /// Get recommended configuration
    pub fn recommended() -> Self {
        let mut rules = HashMap::new();

        // Core validation rules with recommended severities (using CoreRuleId)
        rules.insert(
            CoreRuleId::UndefinedInput.as_ref().to_string(),
            RuleConfig::Severity(Severity::Error)
        );

        // CLI-specific rules (using CliRuleId)
        rules.insert(
            CliRuleId::CliInputOverride.as_ref().to_string(),
            RuleConfig::Severity(Severity::Info)
        );
        rules.insert(
            CliRuleId::InputNamingConvention.as_ref().to_string(),
            RuleConfig::Severity(Severity::Warning)
        );
        rules.insert(
            CliRuleId::NoSensitiveData.as_ref().to_string(),
            RuleConfig::Severity(Severity::Warning)
        );

        Self {
            extends: None,
            rules,
            ignore: vec![],
        }
    }

    /// Get rule configuration for a specific rule
    pub fn get_rule_config(&self, rule_id: &str) -> Option<&RuleConfig> {
        self.rules.get(rule_id)
    }

    /// Check if a rule is disabled
    pub fn is_rule_disabled(&self, rule_id: &str) -> bool {
        self.get_rule_config(rule_id)
            .map(|config| config.is_disabled())
            .unwrap_or(false)
    }

    /// Get severity for a rule (returns None if rule is disabled or not configured)
    pub fn get_rule_severity(&self, rule_id: &str) -> Option<Severity> {
        self.get_rule_config(rule_id)
            .and_then(|config| {
                let severity = config.severity();
                if severity == Severity::Off {
                    None
                } else {
                    Some(severity)
                }
            })
    }
}

#[derive(Clone, Debug)]
pub struct LinterConfig {
    pub manifest_path: Option<PathBuf>,
    /// Runbook name (stored for potential future use, currently passed directly to lint methods)
    #[allow(dead_code)]
    pub runbook: Option<String>,
    pub environment: Option<String>,
    pub cli_inputs: Vec<(String, String)>,
    pub format: Format,
    pub config_file: Option<ConfigFile>,
}

impl LinterConfig {
    pub fn new(
        manifest_path: Option<PathBuf>,
        runbook: Option<String>,
        environment: Option<String>,
        cli_inputs: Vec<(String, String)>,
        format: Format,
    ) -> Self {
        Self {
            manifest_path,
            runbook,
            environment,
            cli_inputs,
            format,
            config_file: None,
        }
    }

    /// Create a new config with a loaded configuration file
    pub fn with_config_file(
        manifest_path: Option<PathBuf>,
        runbook: Option<String>,
        environment: Option<String>,
        cli_inputs: Vec<(String, String)>,
        format: Format,
        config_path: Option<&str>,
    ) -> Self {
        let config_file = ConfigFile::load(config_path)
            .map(|config| config.with_extends());

        Self {
            manifest_path,
            runbook,
            environment,
            cli_inputs,
            format,
            config_file,
        }
    }

    /// Check if a rule is disabled in the configuration
    pub fn is_rule_disabled(&self, rule_id: &str) -> bool {
        self.config_file
            .as_ref()
            .map(|config| config.is_rule_disabled(rule_id))
            .unwrap_or(false)
    }

    /// Get the configured severity for a rule
    pub fn get_rule_severity(&self, rule_id: &str) -> Option<Severity> {
        self.config_file
            .as_ref()
            .and_then(|config| config.get_rule_severity(rule_id))
    }
}

impl Default for LinterConfig {
    fn default() -> Self {
        Self {
            manifest_path: None,
            runbook: None,
            environment: None,
            cli_inputs: Vec::new(),
            format: Format::Stylish,
            config_file: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{TempDir, NamedTempFile};

    #[test]
    fn test_config_file_not_found_returns_none() {
        // Arrange - use nonexistent path
        let nonexistent_path = "/nonexistent/.txtxlint.yml";

        // Act
        let result = ConfigFile::load(Some(nonexistent_path));

        // Assert
        assert!(result.is_none(), "Should return None for nonexistent config file");
    }

    #[test]
    fn test_config_file_malformed_yaml_error() {
        // Arrange - create temp file with invalid YAML
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "invalid: yaml: content: [[[").unwrap();

        // Act
        let result = ConfigFile::from_file(temp_file.path());

        // Assert
        assert!(result.is_err(), "Should return error for malformed YAML");
        let error_msg = result.unwrap_err();
        assert!(error_msg.contains("Failed to parse YAML"),
            "Error should mention YAML parsing: {}", error_msg);
    }

    #[test]
    fn test_config_extends_recommended() {
        // Arrange - create config file with extends
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
extends: "txtx:recommended"
rules:
  cli_input_override: info
"#).unwrap();

        // Act
        let config = ConfigFile::from_file(temp_file.path()).unwrap();
        let config_with_extends = config.with_extends();

        // Assert
        assert_eq!(config.extends, Some("txtx:recommended".to_string()));
        assert!(config_with_extends.rules.contains_key("undefined_input"),
            "Should include recommended rule 'undefined_input'");
        assert!(config_with_extends.rules.contains_key("cli_input_override"),
            "Should include recommended rule 'cli_input_override'");

        let override_rule = config_with_extends.rules.get("cli_input_override").unwrap();
        assert_eq!(override_rule.severity(), Severity::Info);
    }

    #[test]
    fn test_config_rule_severity_override() {
        // Arrange - create config with severity overrides
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
rules:
  undefined_input: warning
  cli_input_override: off
"#).unwrap();

        // Act
        let config = ConfigFile::from_file(temp_file.path()).unwrap();

        // Assert
        let undefined_rule = config.rules.get("undefined_input").unwrap();
        assert_eq!(undefined_rule.severity(), Severity::Warning);

        let override_rule = config.rules.get("cli_input_override").unwrap();
        assert_eq!(override_rule.severity(), Severity::Off);
        assert!(override_rule.is_disabled());
    }

    #[test]
    fn test_config_ignore_patterns() {
        // Arrange - create config with ignore patterns
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
ignore:
  - "examples/**"
  - "tests/**"
  - "**/deprecated/**"
"#).unwrap();

        // Act
        let config = ConfigFile::from_file(temp_file.path()).unwrap();

        // Assert
        assert_eq!(config.ignore.len(), 3);
        assert!(config.ignore.contains(&"examples/**".to_string()));
        assert!(config.ignore.contains(&"tests/**".to_string()));
        assert!(config.ignore.contains(&"**/deprecated/**".to_string()));
    }

    #[test]
    fn test_config_full_rule_config_with_options() {
        // Arrange - create config with rule options
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
rules:
  input_naming_convention:
    severity: warning
    options:
      convention: SCREAMING_SNAKE_CASE
      allow_leading_underscore: false
"#).unwrap();

        // Act
        let config = ConfigFile::from_file(temp_file.path()).unwrap();

        // Assert
        let naming_rule = config.rules.get("input_naming_convention").unwrap();
        assert_eq!(naming_rule.severity(), Severity::Warning);

        let options = naming_rule.options().expect("Should have options");
        assert!(options.contains_key("convention"));
        assert!(options.contains_key("allow_leading_underscore"));
    }

    #[test]
    fn test_linter_config_with_nonexistent_file() {
        // Arrange - use nonexistent config file path
        let nonexistent_path = "/nonexistent/.txtxlint.yml";

        // Act
        let config = LinterConfig::with_config_file(
            None,
            None,
            None,
            vec![],
            Format::Json,
            Some(nonexistent_path),
        );

        // Assert
        assert!(config.config_file.is_none(),
            "Should gracefully handle missing config file");
    }

    #[test]
    fn test_linter_config_rule_disabled_check() {
        // Arrange - create config file with disabled and enabled rules
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join(".txtxlint.yml");
        std::fs::write(&config_path, r#"
rules:
  cli_input_override: off
  input_naming_convention: warning
"#).unwrap();

        // Act
        let config = LinterConfig::with_config_file(
            None,
            None,
            None,
            vec![],
            Format::Json,
            Some(config_path.to_str().unwrap()),
        );

        // Assert
        assert!(config.is_rule_disabled("cli_input_override"),
            "Rule with severity 'off' should be disabled");
        assert!(!config.is_rule_disabled("input_naming_convention"),
            "Rule with severity 'warning' should not be disabled");
        assert!(!config.is_rule_disabled("undefined_rule"),
            "Unconfigured rule should not be disabled");
    }

    #[test]
    fn test_linter_config_get_rule_severity() {
        // Arrange - create config file with various severity levels
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join(".txtxlint.yml");
        std::fs::write(&config_path, r#"
rules:
  undefined_input: error
  cli_input_override: off
  input_naming_convention: info
"#).unwrap();

        // Act
        let config = LinterConfig::with_config_file(
            None,
            None,
            None,
            vec![],
            Format::Json,
            Some(config_path.to_str().unwrap()),
        );

        // Assert
        assert_eq!(config.get_rule_severity("undefined_input"), Some(Severity::Error));
        assert_eq!(config.get_rule_severity("cli_input_override"), None,
            "Disabled rule should return None for severity");
        assert_eq!(config.get_rule_severity("input_naming_convention"), Some(Severity::Info));
        assert_eq!(config.get_rule_severity("undefined_rule"), None,
            "Unconfigured rule should return None");
    }

    #[test]
    fn test_config_load_default_finds_yml() {
        // Arrange - create temp directory with config file
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();
        std::fs::write(".txtxlint.yml", r#"
rules:
  undefined_input: error
"#).unwrap();

        // Act
        let config = ConfigFile::load_default();

        // Cleanup
        std::env::set_current_dir(original_dir).unwrap();

        // Assert
        assert!(config.is_some(), "Should find .txtxlint.yml in current directory");
        let config = config.unwrap();
        assert!(config.rules.contains_key("undefined_input"));
    }

    #[test]
    fn test_recommended_config_has_all_default_rules() {
        // Arrange - no setup needed

        // Act
        let recommended = ConfigFile::recommended();

        // Assert - should have all default rules
        assert!(recommended.rules.contains_key("undefined_input"),
            "Recommended config should include undefined_input");
        assert!(recommended.rules.contains_key("cli_input_override"),
            "Recommended config should include cli_input_override");
        assert!(recommended.rules.contains_key("input_naming_convention"),
            "Recommended config should include input_naming_convention");
        assert!(recommended.rules.contains_key("no_sensitive_data"),
            "Recommended config should include no_sensitive_data");
    }

    #[test]
    fn test_extends_merges_without_overriding_user_config() {
        // Arrange - create config that extends recommended with overrides
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"
extends: "txtx:recommended"
rules:
  undefined_input: warning  # Override recommended 'error'
  custom_rule: info         # User-defined rule
"#).unwrap();

        // Act
        let config = ConfigFile::from_file(temp_file.path()).unwrap();
        let config_with_extends = config.with_extends();

        // Assert - user overrides should win
        let undefined_rule = config_with_extends.rules.get("undefined_input").unwrap();
        assert_eq!(undefined_rule.severity(), Severity::Warning,
            "User config should override recommended");

        assert!(config_with_extends.rules.contains_key("cli_input_override"),
            "Non-overridden recommended rules should be present");

        assert!(config_with_extends.rules.contains_key("custom_rule"),
            "User-defined rules should be preserved");
    }
}
