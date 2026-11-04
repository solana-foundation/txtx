use clap::{ArgAction, Parser, Subcommand};
use dotenvy::dotenv;
use txtx_system_kit::{self, Logger};
use std::io::IsTerminal;
use std::process;

mod common;
mod docs;
mod lint;
mod lsp;
mod runbooks;
mod snapshots;

#[derive(Clone)]
pub struct Context {
    pub logger: Option<Logger>,
    pub tracer: bool,
}

#[allow(dead_code)]
impl Context {
    pub fn empty() -> Context {
        Context { logger: None, tracer: false }
    }

    pub fn try_log<F>(&self, closure: F)
    where
        F: FnOnce(&Logger),
    {
        if let Some(ref logger) = self.logger {
            closure(logger)
        }
    }

    pub fn expect_logger(&self) -> &Logger {
        self.logger.as_ref().unwrap()
    }
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Opts {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, PartialEq, Clone, Debug)]
enum Command {
    /// List the runbooks indexed in the txtx manifest
    #[clap(name = "ls", bin_name = "ls")]
    List(ListRunbooks),
    /// Create a new runbook
    #[clap(name = "new", bin_name = "new")]
    New(CreateRunbook),
    /// Check a runbook against a previous execution's statefile to list which actions will be re-executed
    #[clap(name = "check", bin_name = "check")]
    Check(CheckRunbook),
    /// Execute a runbook. Run, runbook, run!
    #[clap(name = "run", bin_name = "run")]
    Run(ExecuteRunbook),
    /// Display documentation
    #[clap(name = "docs", bin_name = "docs")]
    Docs(GetDocumentation),
    /// Lint runbooks for issues and style violations
    #[clap(name = "lint", bin_name = "lint")]
    Lint(LintRunbook),
    /// Start the txtx language server
    #[clap(name = "lsp", bin_name = "lsp")]
    Lsp,
    /// Snapshot management (work in progress)
    #[clap(subcommand)]
    Snapshots(SnapshotCommand),
}

#[derive(Subcommand, PartialEq, Clone, Debug)]
enum SnapshotCommand {
    /// Begin new snapshot
    #[clap(name = "begin", bin_name = "begin")]
    Begin(BeginSnapshot),
    /// Finalize snapshot
    #[clap(name = "end", bin_name = "end")]
    Commit(CommitSnapshot),
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct BeginSnapshot {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,
    /// Path to the snapshot
    #[arg(long = "snapshot-file-path", short = 's')]
    pub snapshot_path: String,
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct CommitSnapshot {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,
    /// Path to the snapshot
    #[arg(long = "snapshot-file-path", short = 's')]
    pub snapshot_path: String,
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct CheckRunbook {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,
    /// Name of the runbook as indexed in the txtx.yml, or the path of the .tx file to run
    pub runbook: String,
    /// Choose the environment variable to set from those configured in the txtx.yml
    #[arg(long = "env")]
    pub environment: Option<String>,
    /// A set of inputs to use for batch processing
    #[arg(long = "input")]
    pub inputs: Vec<String>,
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct GetDocumentation;

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct InspectRunbook {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,
    /// Disable Terminal UI
    #[clap(long = "no-term-ui")]
    pub no_tui: bool,
    /// Path to runbook root file
    // #[clap(long = "runbook-path", conflicts_with = "runbook")]
    // pub runbook_path: Option<String>,
    /// Name of runbook as indexed in txtx.yml
    #[clap(long = "runbook")]
    pub runbook: Option<String>,
}

#[derive(Parser, PartialEq, Clone, Debug)]
#[command(group = clap::ArgGroup::new("execution_mode").multiple(false).args(["unsupervised", "web_console", "term_console"]).required(false))]
pub struct ExecuteRunbook {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,
    /// Name of the runbook as indexed in the txtx.yml, or the path of the .tx file to run
    pub runbook: String,

    /// Execute the runbook without supervision
    #[arg(long = "unsupervised", short = 'u', action=ArgAction::SetTrue, group = "execution_mode")]
    pub unsupervised: bool,
    /// Execute the runbook with supervision via the browser UI (this is the default execution mode)
    #[arg(long = "browser", short = 'b', action=ArgAction::SetTrue, group = "execution_mode")]
    pub web_console: bool,
    /// Execute the runbook with supervision via the terminal console (coming soon)
    #[arg(long = "terminal", short = 't', action=ArgAction::SetTrue, group = "execution_mode")]
    pub term_console: bool,
    /// When running in unsupervised mode, print outputs in JSON format. If a directory is provided, the output will be written a file at the directory.
    #[arg(long = "output-json")]
    pub output_json: Option<Option<String>>,
    /// Pick a specific output to stdout at the end of the execution
    #[arg(long = "output", conflicts_with = "output_json")]
    pub output: Option<String>,
    /// Explain how the runbook will be executed.
    #[arg(long = "explain", action=ArgAction::SetTrue)]
    pub explain: bool,
    /// Set the port for hosting the web UI
    #[arg(long = "port", short = 'p', default_value = txtx_supervisor_ui::DEFAULT_BINDING_PORT )]
    #[cfg(feature = "supervisor_ui")]
    pub network_binding_port: u16,
    /// Set the port for hosting the web UI
    #[arg(long = "ip", short = 'i', default_value = txtx_supervisor_ui::DEFAULT_BINDING_ADDRESS )]
    #[cfg(feature = "supervisor_ui")]
    pub network_binding_ip_address: String,
    /// Choose the environment variable to set from those configured in the txtx.yml
    #[arg(long = "env")]
    pub environment: Option<String>,
    /// A set of inputs to use for batch processing
    #[arg(long = "input")]
    pub inputs: Vec<String>,

    /// Execute the Runbook even if the cached state suggests this Runbook has already been executed
    #[arg(long = "force", short = 'f')]
    pub force_execution: bool,
    /// The log level to use for the runbook execution. Options are "trace", "debug", "info", "warn", "error".
    #[arg(long = "log-level", short = 'l', default_value = "info")]
    pub log_level: String,
}

impl ExecuteRunbook {
    pub fn do_start_supervisor_ui(&self) -> bool {
        self.web_console || (!self.unsupervised && !self.term_console)
    }
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct CreateRunbook {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct ListRunbooks {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm', default_value = "./txtx.yml")]
    pub manifest_path: String,

    /// List environments instead of runbooks
    #[arg(long = "envs", alias = "env", short = 'e')]
    pub envs: bool,
}

#[derive(Parser, PartialEq, Clone, Debug)]
pub struct LintRunbook {
    /// Path to the manifest
    #[arg(long = "manifest-file-path", short = 'm')]
    pub manifest_path: Option<String>,
    /// Name of the runbook to lint (omit to lint all runbooks)
    pub runbook: Option<String>,
    /// Choose the environment variable to set from those configured in the txtx.yml
    #[arg(long = "env")]
    pub environment: Option<String>,
    /// A set of inputs to use for linting
    #[arg(long = "input")]
    pub inputs: Vec<String>,
    /// Output format
    #[arg(long = "format", short = 'f', default_value = "stylish")]
    pub format: lint::Format,
    /// Path to linter config file
    #[arg(long = "config")]
    pub config: Option<String>,
    /// Disable specific rules (can be used multiple times)
    #[arg(long = "disable-rule")]
    pub disabled_rules: Vec<String>,
    /// Only run specific rules (can be used multiple times)
    #[arg(long = "only-rule")]
    pub only_rules: Vec<String>,
    /// Automatically fix issues where possible
    #[arg(long = "fix")]
    pub fix: bool,
    /// Initialize a new linter configuration file
    #[arg(long = "init")]
    pub init: bool,
    /// Generate CLI command template for undefined inputs
    #[arg(long = "gen-cli")]
    pub gen_cli: bool,
    /// Generate full CLI command template with all inputs
    #[arg(long = "gen-cli-full")]
    pub gen_cli_full: bool,
}

fn load_stdin() -> Option<String> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let mut buffer = String::new();
    std::io::stdin().read_line(&mut buffer).ok()?;
    return Some(buffer);
}

pub fn main() {
    let logger = txtx_system_kit::log::setup_logger();
    let _guard = txtx_system_kit::log::setup_global_logger(logger.clone());
    let ctx = Context { logger: Some(logger), tracer: false };

    let opts: Opts = match Opts::try_parse() {
        Ok(opts) => opts,
        Err(e) => e.exit(),
    };

    let buffer_stdin = if let Command::Lsp = opts.command { None } else { load_stdin() };

    match txtx_system_kit::nestable_block_on(handle_command(opts, &ctx, buffer_stdin)) {
        Err(e) => {
            error!(ctx.expect_logger(), "{e}");
            std::thread::sleep(std::time::Duration::from_millis(500));
            process::exit(1);
        }
        Ok(_) => {}
    }
}

async fn handle_command(
    opts: Opts,
    ctx: &Context,
    buffer_stdin: Option<String>,
) -> Result<(), String> {
    match opts.command {
        Command::Check(cmd) => {
            runbooks::handle_check_command(&cmd, buffer_stdin, ctx).await?;
        }
        Command::Run(cmd) => {
            runbooks::handle_run_command(&cmd, buffer_stdin, ctx).await?;
        }
        Command::List(cmd) => {
            runbooks::handle_list_command(&cmd, ctx).await?;
        }
        Command::New(cmd) => {
            runbooks::handle_new_command(&cmd, ctx).await?;
        }
        Command::Docs(cmd) => {
            docs::handle_docs_command(&cmd, ctx).await?;
        }
        Command::Lint(cmd) => {
            handle_lint_command(&cmd).map_err(|e| e.to_string())?;
        }
        Command::Snapshots(SnapshotCommand::Begin(cmd)) => {
            snapshots::handle_begin_command(&cmd, ctx).await?;
        }
        Command::Snapshots(SnapshotCommand::Commit(cmd)) => {
            snapshots::handle_commit_command(&cmd, ctx).await?;
        }
        Command::Lsp => {
            lsp::run_lsp().await?;
        }
    }
    Ok(())
}

fn handle_lint_command(cmd: &LintRunbook) -> Result<(), lint::LinterError> {
    // Parse CLI inputs from "key=value" strings
    let cli_inputs: Vec<(String, String)> = cmd
        .inputs
        .iter()
        .filter_map(|input| {
            let parts: Vec<&str> = input.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect();

    let linter_options = lint::LinterOptions { config_path: cmd.config.clone(), init: cmd.init };

    lint::run_lint(
        cmd.runbook.clone(),
        cmd.manifest_path.clone(),
        cmd.environment.clone(),
        cli_inputs,
        cmd.format.clone(),
        linter_options,
        cmd.gen_cli,
        cmd.gen_cli_full,
    )
}

pub fn get_env_var<T: ToString>(key: &str, default: T) -> String {
    dotenv().ok();
    std::env::var(key).unwrap_or(default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn parse_args(args: Vec<&str>) -> ExecuteRunbook {
        ExecuteRunbook::parse_from(args)
    }

    #[test]
    fn test_execute_runbook_default_values() {
        let args = vec!["txtx", "runbook"];
        let result = parse_args(args);
        assert_eq!(result.manifest_path, "./txtx.yml");
        assert_eq!(result.runbook, "runbook");
        assert_eq!(result.unsupervised, false);
        assert_eq!(result.web_console, false);
        assert_eq!(result.term_console, false);
        #[cfg(feature = "supervisor_ui")]
        assert_eq!(result.network_binding_port, 8488);
        #[cfg(feature = "supervisor_ui")]
        assert_eq!(result.network_binding_ip_address, "localhost");
        assert_eq!(result.environment, None);
        assert!(result.inputs.is_empty());
    }

    #[test]
    fn test_unsupervised_mode() {
        let args = vec!["txtx", "runbook", "--unsupervised"];
        let result = parse_args(args);
        assert_eq!(result.unsupervised, true);
        assert_eq!(result.web_console, false);
        assert_eq!(result.term_console, false);
    }

    #[test]
    fn test_web_console_mode() {
        let args = vec!["txtx", "runbook", "--browser"];
        let result = parse_args(args);
        assert_eq!(result.unsupervised, false);
        assert_eq!(result.web_console, true);
        assert_eq!(result.term_console, false);
    }

    #[test]
    fn test_terminal_console_mode() {
        let args = vec!["txtx", "runbook", "--terminal"];
        let result = parse_args(args);
        assert_eq!(result.unsupervised, false);
        assert_eq!(result.web_console, false);
        assert_eq!(result.term_console, true);
    }

    #[test]
    #[cfg(feature = "supervisor_ui")]
    fn test_port_setting() {
        let args = vec!["txtx", "runbook", "--port", "9090"];
        let result = parse_args(args);
        assert_eq!(result.network_binding_port, 9090);
    }

    #[test]
    #[cfg(feature = "supervisor_ui")]
    fn test_ip_setting() {
        let args = vec!["txtx", "runbook", "--ip", "192.168.1.10"];
        let result = parse_args(args);
        assert_eq!(result.network_binding_ip_address, "192.168.1.10");
    }

    #[test]
    fn test_environment_setting() {
        let args = vec!["txtx", "runbook", "--env", "production"];
        let result = parse_args(args);
        assert_eq!(result.environment, Some(String::from("production")));
    }

    #[test]
    fn test_inputs_setting() {
        let args = vec!["txtx", "runbook", "--input", "input1", "--input", "input2"];
        let result = parse_args(args);
        assert_eq!(result.inputs, vec!["input1", "input2"]);
    }

    #[test_case("--unsupervised", "--browser")]
    #[test_case("--unsupervised", "--terminal")]
    #[test_case("--browser", "--terminal")]
    fn test_conflicting_arguments(arg1: &str, arg2: &str) {
        let args = vec!["txtx", "runbook", arg1, arg2];
        let thing = ExecuteRunbook::try_parse_from(args);
        let err = thing.unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}
