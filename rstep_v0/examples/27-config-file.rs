//! Example 27: Config File
//!
//! Demonstrates loading step configuration from `.rloop/config.toml`.
//!
//! This example shows:
//! 1. Define config structure matching RSTEP.md specification
//! 2. Load from `.rloop/config.toml` if present
//! 3. Fall back to sensible defaults when not found
//! 4. Use config values for step execution
//!
//! Config file format (`.rloop/config.toml`):
//! ```toml
//! [step]
//! model = "claude-sonnet-4-20250514"
//! max_turns = 50
//!
//! [step.tools]
//! allowed = ["Read", "Write", "Edit", "Bash", "Glob", "Grep"]
//!
//! [verification]
//! commands = ["cargo check", "cargo test"]
//! fail_fast = true
//!
//! [worktree]
//! root = ".rloop/worktrees"
//! auto_merge = true
//!
//! [logging]
//! session_dir = ".rloop/sessions"
//! level = "debug"
//! ```
//!
//! Run with:
//!   cargo run --example 27-config-file
//!
//! Key additions:
//!   - Config structs with serde defaults
//!   - Config loading from TOML file
//!   - Fallback to defaults when no config file

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use serde::Deserialize;
use std::fs;
use std::path::Path;
use tempfile::tempdir;
use tracing::info;

// ============================================================
// CONFIG TYPES
// ============================================================

/// Root configuration structure.
///
/// All sections are optional and have sensible defaults.
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub step: StepConfig,

    #[serde(default)]
    pub verification: VerificationConfig,

    #[serde(default)]
    pub worktree: WorktreeConfig,

    #[serde(default)]
    pub logging: LoggingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            step: StepConfig::default(),
            verification: VerificationConfig::default(),
            worktree: WorktreeConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

/// Step execution configuration.
#[derive(Debug, Deserialize)]
pub struct StepConfig {
    /// Model to use for Claude queries.
    #[serde(default = "default_model")]
    pub model: String,

    /// Maximum number of turns (tool use rounds) per step.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Tools configuration.
    #[serde(default)]
    pub tools: ToolsConfig,
}

impl Default for StepConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            max_turns: default_max_turns(),
            tools: ToolsConfig::default(),
        }
    }
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_max_turns() -> u32 {
    50
}

/// Tools configuration.
#[derive(Debug, Deserialize)]
pub struct ToolsConfig {
    /// Explicitly allowed tools.
    #[serde(default = "default_allowed_tools")]
    pub allowed: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            allowed: default_allowed_tools(),
        }
    }
}

fn default_allowed_tools() -> Vec<String> {
    vec![
        "Read".to_string(),
        "Write".to_string(),
        "Edit".to_string(),
        "Bash".to_string(),
        "Glob".to_string(),
        "Grep".to_string(),
    ]
}

/// Verification configuration.
#[derive(Debug, Deserialize)]
pub struct VerificationConfig {
    /// Commands to run for verification.
    #[serde(default = "default_verification_commands")]
    pub commands: Vec<String>,

    /// Stop on first failure.
    #[serde(default = "default_fail_fast")]
    pub fail_fast: bool,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            commands: default_verification_commands(),
            fail_fast: default_fail_fast(),
        }
    }
}

fn default_verification_commands() -> Vec<String> {
    vec!["cargo check".to_string(), "cargo test".to_string()]
}

fn default_fail_fast() -> bool {
    true
}

/// Worktree configuration.
#[derive(Debug, Deserialize)]
pub struct WorktreeConfig {
    /// Root directory for worktrees.
    #[serde(default = "default_worktree_root")]
    pub root: String,

    /// Automatically merge on success.
    #[serde(default = "default_auto_merge")]
    pub auto_merge: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            root: default_worktree_root(),
            auto_merge: default_auto_merge(),
        }
    }
}

fn default_worktree_root() -> String {
    ".rloop/worktrees".to_string()
}

fn default_auto_merge() -> bool {
    true
}

/// Logging configuration.
#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    /// Directory for session logs.
    #[serde(default = "default_session_dir")]
    pub session_dir: String,

    /// Log level.
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            session_dir: default_session_dir(),
            level: default_log_level(),
        }
    }
}

fn default_session_dir() -> String {
    ".rloop/sessions".to_string()
}

fn default_log_level() -> String {
    "debug".to_string()
}

// ============================================================
// CONFIG LOADING
// ============================================================

/// Result of attempting to load config.
#[derive(Debug)]
pub enum ConfigLoadResult {
    /// Loaded from file.
    Loaded(Config),
    /// Using defaults (file not found).
    Defaults(Config),
    /// Error reading or parsing file.
    Error(String),
}

/// Load config from `.rloop/config.toml` in the given directory.
///
/// If the file doesn't exist, returns defaults.
/// If the file exists but can't be parsed, returns an error.
pub fn load_config(base_dir: &Path) -> ConfigLoadResult {
    let config_path = base_dir.join(".rloop/config.toml");

    if !config_path.exists() {
        return ConfigLoadResult::Defaults(Config::default());
    }

    match fs::read_to_string(&config_path) {
        Ok(contents) => match toml::from_str::<Config>(&contents) {
            Ok(config) => ConfigLoadResult::Loaded(config),
            Err(e) => ConfigLoadResult::Error(format!("Failed to parse config: {}", e)),
        },
        Err(e) => ConfigLoadResult::Error(format!("Failed to read config: {}", e)),
    }
}

/// Load config, returning defaults on any error.
pub fn load_config_or_default(base_dir: &Path) -> Config {
    match load_config(base_dir) {
        ConfigLoadResult::Loaded(c) | ConfigLoadResult::Defaults(c) => c,
        ConfigLoadResult::Error(_) => Config::default(),
    }
}

// ============================================================
// DISPLAY HELPERS
// ============================================================

fn print_config(config: &Config, source: &str) {
    println!("\n{}", "Configuration:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!("  {} {}", "Source:".dimmed(), source.cyan());

    println!("\n  {}", "[step]".yellow().bold());
    println!("    {} {}", "model:".dimmed(), config.step.model.white());
    println!(
        "    {} {}",
        "max_turns:".dimmed(),
        config.step.max_turns.to_string().white()
    );

    println!("\n  {}", "[step.tools]".yellow().bold());
    println!(
        "    {} {}",
        "allowed:".dimmed(),
        format!("{:?}", config.step.tools.allowed).white()
    );

    println!("\n  {}", "[verification]".yellow().bold());
    println!(
        "    {} {}",
        "commands:".dimmed(),
        format!("{:?}", config.verification.commands).white()
    );
    println!(
        "    {} {}",
        "fail_fast:".dimmed(),
        config.verification.fail_fast.to_string().white()
    );

    println!("\n  {}", "[worktree]".yellow().bold());
    println!(
        "    {} {}",
        "root:".dimmed(),
        config.worktree.root.white()
    );
    println!(
        "    {} {}",
        "auto_merge:".dimmed(),
        config.worktree.auto_merge.to_string().white()
    );

    println!("\n  {}", "[logging]".yellow().bold());
    println!(
        "    {} {}",
        "session_dir:".dimmed(),
        config.logging.session_dir.white()
    );
    println!("    {} {}", "level:".dimmed(), config.logging.level.white());

    println!("{}", "-".repeat(60).dimmed());
}

// ============================================================
// MAIN
// ============================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    println!(
        "\n{}",
        "+------------------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                      CONFIG FILE LOADING                         |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                                  |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  Load step configuration from .rloop/config.toml                 |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+------------------------------------------------------------------+"
            .cyan()
            .bold()
    );

    // ============================================================
    // DEMO 1: Load from current directory (likely no config)
    // ============================================================

    println!(
        "\n{}",
        "1. Loading config from current directory...".blue().bold()
    );

    let cwd = std::env::current_dir()?;
    match load_config(&cwd) {
        ConfigLoadResult::Loaded(config) => {
            print_config(&config, &format!("{}/.rloop/config.toml", cwd.display()));
        }
        ConfigLoadResult::Defaults(config) => {
            println!(
                "\n  {} No config file found, using defaults",
                "INFO".yellow().bold()
            );
            print_config(&config, "defaults");
        }
        ConfigLoadResult::Error(e) => {
            println!("\n  {} {}", "ERROR".red().bold(), e);
        }
    }

    // ============================================================
    // DEMO 2: Create a temp directory with custom config
    // ============================================================

    println!(
        "\n{}",
        "2. Creating temp directory with custom config...".blue().bold()
    );

    let temp = tempdir()?;

    // Create .rloop directory and config file
    let rloop_dir = temp.path().join(".rloop");
    fs::create_dir_all(&rloop_dir)?;

    let custom_config = r#"# Custom rstep configuration

[step]
model = "claude-haiku-4-5-20251001"
max_turns = 25

[step.tools]
allowed = ["Read", "Edit"]

[verification]
commands = ["cargo check"]
fail_fast = false

[worktree]
root = ".custom/worktrees"
auto_merge = false

[logging]
session_dir = ".custom/logs"
level = "info"
"#;

    let config_path = rloop_dir.join("config.toml");
    fs::write(&config_path, custom_config)?;

    println!(
        "  {} {}",
        "Created:".green(),
        config_path.display().to_string().white()
    );
    println!("\n  {}", "Config contents:".yellow().bold());
    for line in custom_config.lines() {
        println!("    {}", line.dimmed());
    }

    // Load the custom config
    match load_config(temp.path()) {
        ConfigLoadResult::Loaded(config) => {
            print_config(&config, &format!("{}", config_path.display()));

            // ============================================================
            // DEMO 3: Use config values in Claude call
            // ============================================================

            println!(
                "\n{}",
                "3. Using config values for Claude call...".blue().bold()
            );
            println!("{}", "-".repeat(60).dimmed());

            println!(
                "  {} {}",
                "Model from config:".dimmed(),
                config.step.model.cyan().bold()
            );
            println!(
                "  {} {:?}",
                "Tools from config:".dimmed(),
                config.step.tools.allowed
            );

            // Configure Claude using values from config
            let options = ClaudeAgentOptions::builder()
                .model(&config.step.model)
                .allowed_tools(config.step.tools.allowed.clone())
                .permission_mode(PermissionMode::BypassPermissions)
                .cwd(temp.path())
                .build();

            let mut client = ClaudeClient::new(options);
            client.connect().await?;

            // Simple query to verify config works
            println!(
                "\n  {} {}",
                "Sending test query with config model...".yellow(),
                config.step.model.cyan()
            );

            client.query("What is 2+2? Reply with just the number.").await?;

            let mut response_text = String::new();
            let mut total_tokens: u64 = 0;

            {
                let mut stream = client.receive_response();
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(message) => match &message {
                            Message::Assistant(msg) => {
                                for block in &msg.message.content {
                                    if let claude_agent_sdk_rs::ContentBlock::Text(text_block) =
                                        block
                                    {
                                        response_text.push_str(&text_block.text);
                                    }
                                }
                                if let Some(usage) = &msg.message.usage {
                                    if let Some(input) =
                                        usage.get("input_tokens").and_then(|v| v.as_u64())
                                    {
                                        total_tokens += input;
                                    }
                                    if let Some(output) =
                                        usage.get("output_tokens").and_then(|v| v.as_u64())
                                    {
                                        total_tokens += output;
                                    }
                                }
                            }
                            Message::Result(result) => {
                                if let Some(usage) = &result.usage {
                                    if let Some(input) =
                                        usage.get("input_tokens").and_then(|v| v.as_u64())
                                    {
                                        total_tokens += input;
                                    }
                                    if let Some(output) =
                                        usage.get("output_tokens").and_then(|v| v.as_u64())
                                    {
                                        total_tokens += output;
                                    }
                                }
                            }
                            _ => {}
                        },
                        Err(e) => {
                            eprintln!("  {} {}", "Error:".red().bold(), e);
                        }
                    }
                }
            }

            client.disconnect().await?;

            println!("\n  {} {}", "Response:".green(), response_text.trim());
            println!("  {} {}", "Tokens:".dimmed(), total_tokens);
        }
        ConfigLoadResult::Defaults(_) => {
            println!("  {} Config file not found (unexpected)", "ERROR".red().bold());
        }
        ConfigLoadResult::Error(e) => {
            println!("  {} {}", "ERROR".red().bold(), e);
        }
    }

    // ============================================================
    // DEMO 4: Partial config (only some sections)
    // ============================================================

    println!(
        "\n{}",
        "4. Testing partial config (only some sections)...".blue().bold()
    );

    let partial_config = r#"# Only override the model
[step]
model = "claude-opus-4-20250514"
"#;

    fs::write(&config_path, partial_config)?;

    println!("  {}", "Config with only [step] section:".yellow());
    for line in partial_config.lines() {
        println!("    {}", line.dimmed());
    }

    match load_config(temp.path()) {
        ConfigLoadResult::Loaded(config) => {
            println!(
                "\n  {} Partial config loaded, other sections use defaults",
                "SUCCESS".green().bold()
            );
            print_config(&config, "partial config + defaults");
        }
        ConfigLoadResult::Error(e) => {
            println!("  {} {}", "ERROR".red().bold(), e);
        }
        _ => {}
    }

    // ============================================================
    // SUMMARY
    // ============================================================

    println!("\n{}", "=".repeat(60).cyan().bold());
    println!(
        "{}",
        "                       CONFIG SUMMARY                        "
            .cyan()
            .bold()
    );
    println!("{}", "=".repeat(60).cyan().bold());

    println!(
        "\n  {} Config file location:",
        "Location:".white().bold()
    );
    println!("    {}", ".rloop/config.toml".cyan());

    println!("\n  {} Config sections:", "Sections:".white().bold());
    println!("    {} Model, max_turns, tools", "[step]".yellow());
    println!("    {} Verification commands, fail_fast", "[verification]".yellow());
    println!("    {} Worktree root, auto_merge", "[worktree]".yellow());
    println!("    {} Session dir, log level", "[logging]".yellow());

    println!(
        "\n  {} All sections and fields have defaults",
        "Defaults:".white().bold()
    );
    println!("    {} Partial configs work - unspecified values use defaults", "->".green());

    info!("{}", "Done.".green());

    Ok(())
}
