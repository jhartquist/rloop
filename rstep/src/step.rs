//! Step execution module.
//!
//! This module provides the core step execution logic for rstep,
//! implementing the complete flow from RSTEP.md:
//!
//! 1. Load configuration
//! 2. Get task
//! 3. Check for blockers
//! 4. Create worktree
//! 5. Lock worktree
//! 6. Execute Claude
//! 7. Commit changes
//! 8. Verify
//! 9. Merge or rollback
//! 10. Cleanup
//! 11. Log session
//! 12. Return result
//!
//! # Example
//!
//! ```ignore
//! use rstep::step::{run_step, StepConfig, StepResult};
//!
//! let config = StepConfig::default();
//! let result = run_step("bd-task123", &config).await?;
//!
//! match result {
//!     StepResult::Success { summary, .. } => println!("Done: {}", summary),
//!     StepResult::Failed { reason, .. } => println!("Failed: {}", reason),
//!     StepResult::Blocked { blocker } => println!("Blocked: {:?}", blocker),
//! }
//! ```

use crate::git::{
    cleanup_worktree, commit_changes, create_worktree, lock_worktree, merge_to_main,
    unlock_worktree,
};
use crate::human_queue::{create_human_queue_tools, HumanQueue};
use anyhow::{Context, Result};
use chrono::Utc;
use claude_agent_sdk_rs::{
    create_sdk_mcp_server, ClaudeAgentOptions, ClaudeClient, ContentBlock, McpServerConfig,
    McpServers, Message, PermissionMode,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

// ============================================================
// CONFIG TYPES
// ============================================================

/// Step execution configuration.
#[derive(Debug, Clone, Deserialize)]
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

/// Step execution settings.
#[derive(Debug, Clone, Deserialize)]
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
    "claude-haiku-4-5-20251001".to_string()
}

fn default_max_turns() -> u32 {
    50
}

/// Tools configuration.
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
// TASK TYPE
// ============================================================

/// A task to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task ID (e.g., "bd-abc123")
    pub id: String,

    /// Task title
    pub title: String,

    /// Detailed description (optional)
    #[serde(default)]
    pub description: Option<String>,

    /// Task status
    pub status: String,

    /// Priority (0=Critical, 4=Backlog)
    pub priority: i32,

    /// Issue type (task, bug, feature, etc.)
    pub issue_type: String,

    /// Acceptance criteria (optional)
    #[serde(default)]
    pub acceptance_criteria: Option<String>,
}

impl Task {
    /// Get the prompt to send to Claude for this task.
    pub fn to_prompt(&self) -> String {
        let mut prompt = format!("Task: {}\n", self.title);
        if let Some(desc) = &self.description {
            prompt.push_str(&format!("\nDescription:\n{}\n", desc));
        }
        if let Some(ac) = &self.acceptance_criteria {
            prompt.push_str(&format!("\nAcceptance Criteria:\n{}\n", ac));
        }
        prompt
    }
}

// ============================================================
// STEP RESULT TYPES
// ============================================================

/// Blocker types that prevent step completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Blocker {
    /// A blocking question from the human queue.
    Question { id: String, text: String },

    /// An assumption that needs review.
    Assumption { id: String, needs_review: bool },

    /// Blocked on another task.
    Dependency { task_id: String },
}

/// Result of a step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepResult {
    /// Step completed successfully.
    Success {
        summary: String,
        tokens_used: u64,
        duration_ms: u64,
    },

    /// Step failed.
    Failed {
        reason: String,
        next_steps: Vec<String>,
        tokens_used: u64,
    },

    /// Step blocked on human input.
    Blocked { blocker: Blocker },
}

// ============================================================
// CONFIG LOADING
// ============================================================

/// Load config from `.rloop/config.toml` in the given directory.
///
/// Returns defaults if file doesn't exist or can't be parsed.
pub fn load_config(base_dir: &Path) -> Config {
    let config_path = base_dir.join(".rloop/config.toml");

    if !config_path.exists() {
        return Config::default();
    }

    match fs::read_to_string(&config_path) {
        Ok(contents) => toml::from_str::<Config>(&contents).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

// ============================================================
// VERIFICATION
// ============================================================

/// Run verification commands in the given directory.
///
/// Returns `(all_passed, output_log)`.
pub fn run_verification(
    path: &Path,
    commands: &[String],
    fail_fast: bool,
) -> Result<(bool, String)> {
    let mut all_passed = true;
    let mut output = String::new();

    for cmd in commands {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let result = Command::new(parts[0])
            .args(&parts[1..])
            .current_dir(path)
            .output()
            .with_context(|| format!("Failed to run: {}", cmd))?;

        let cmd_output = format!(
            "$ {}\n{}\n",
            cmd,
            String::from_utf8_lossy(&result.stdout)
        );
        output.push_str(&cmd_output);

        if !result.status.success() {
            all_passed = false;
            output.push_str(&format!(
                "FAILED: {}\n{}",
                cmd,
                String::from_utf8_lossy(&result.stderr)
            ));
            if fail_fast {
                break;
            }
        }
    }

    Ok((all_passed, output))
}

// ============================================================
// STEP EXECUTION
// ============================================================

/// Execute a step for the given task.
///
/// This is the core function that implements the complete step flow:
///
/// 1. Create worktree for task isolation
/// 2. Lock worktree
/// 3. Run Claude with human queue tools
/// 4. Commit changes (if any)
/// 5. Run verification
/// 6. On success: merge to main
/// 7. On failure: rollback
/// 8. Cleanup worktree
///
/// # Arguments
///
/// * `repo_path` - Path to the repository
/// * `task` - The task to execute
/// * `config` - Step configuration
///
/// # Returns
///
/// `StepResult` indicating success, failure, or blocked status.
pub async fn run_step(repo_path: &Path, task: &Task, config: &Config) -> Result<StepResult> {
    let start_time = Utc::now();

    // Phase 1: Create worktree
    let worktree_path = create_worktree(repo_path, &task.id)?;

    // Phase 2: Lock worktree
    lock_worktree(&worktree_path, "rstep active")?;

    // Phase 3: Execute Claude
    let (tokens_used, code_changed, queue) =
        execute_claude(&worktree_path, task, config).await?;

    // Check for blocking items
    if queue.has_blocking_items() {
        unlock_worktree(&worktree_path)?;
        let _ = cleanup_worktree(repo_path, &task.id);

        let blocker = if let Some(q) = queue.questions.iter().find(|q| q.blocking) {
            Blocker::Question {
                id: format!("hq-q-{}", Utc::now().timestamp()),
                text: q.question.clone(),
            }
        } else {
            Blocker::Assumption {
                id: format!("hq-a-{}", Utc::now().timestamp()),
                needs_review: true,
            }
        };

        return Ok(StepResult::Blocked { blocker });
    }

    // Phase 4: Commit changes
    let commit_hash = if code_changed {
        match commit_changes(&worktree_path, &format!("fix: {}", task.title)) {
            Ok(hash) => Some(hash),
            Err(_) => None,
        }
    } else {
        None
    };

    // Phase 5: Verify
    let (verification_passed, _output) = run_verification(
        &worktree_path,
        &config.verification.commands,
        config.verification.fail_fast,
    )?;

    let end_time = Utc::now();
    let duration_ms = (end_time - start_time).num_milliseconds() as u64;

    // Phase 6: Merge or rollback
    let result = if code_changed && verification_passed && commit_hash.is_some() {
        // Unlock before merge
        unlock_worktree(&worktree_path)?;

        // Merge to main
        let summary = task.title.to_lowercase();
        merge_to_main(repo_path, &task.id, &summary)?;

        StepResult::Success {
            summary: format!("Completed: {}", task.title),
            tokens_used,
            duration_ms,
        }
    } else if !code_changed {
        unlock_worktree(&worktree_path)?;

        StepResult::Failed {
            reason: "No changes were made to the code".to_string(),
            next_steps: vec![
                "Review the task description".to_string(),
                "Provide more context".to_string(),
            ],
            tokens_used,
        }
    } else {
        unlock_worktree(&worktree_path)?;

        StepResult::Failed {
            reason: "Verification failed".to_string(),
            next_steps: vec![
                "Review the changes".to_string(),
                "Check verification output".to_string(),
            ],
            tokens_used,
        }
    };

    // Phase 7: Cleanup
    let _ = cleanup_worktree(repo_path, &task.id);

    Ok(result)
}

/// Execute Claude in the worktree.
///
/// Returns `(tokens_used, code_changed, human_queue)`.
async fn execute_claude(
    worktree_path: &Path,
    task: &Task,
    config: &Config,
) -> Result<(u64, bool, HumanQueue)> {
    // Create human queue
    let human_queue: Arc<Mutex<HumanQueue>> = Arc::new(Mutex::new(HumanQueue::new()));

    // Create human queue MCP tools
    let hq_tools = create_human_queue_tools(human_queue.clone());
    let mcp_server = create_sdk_mcp_server("rstep-hq", "0.1.0", hq_tools);

    let mut mcp_servers_map: HashMap<String, McpServerConfig> = HashMap::new();
    mcp_servers_map.insert("rstep-hq".to_string(), McpServerConfig::Sdk(mcp_server));

    // Configure Claude
    let options = ClaudeAgentOptions::builder()
        .model(&config.step.model)
        .allowed_tools(config.step.tools.allowed.clone())
        .mcp_servers(McpServers::Dict(mcp_servers_map))
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(worktree_path)
        .build();

    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // Build the prompt
    let prompt = format!(
        r#"{}

Complete this task. Read the relevant files, make necessary changes, and verify with tests.

You have access to human queue tools if needed:
- mcp__rstep-hq__ask_question - Ask if requirements are unclear
- mcp__rstep-hq__log_assumption - Log decisions for review
- mcp__rstep-hq__propose_task - Suggest follow-up work

When done, summarize what you changed."#,
        task.to_prompt()
    );

    client.query(&prompt).await?;

    // Receive response
    let mut total_tokens: u64 = 0;
    let mut tool_uses: Vec<String> = Vec::new();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => match &message {
                    Message::Assistant(msg) => {
                        for block in &msg.message.content {
                            if let ContentBlock::ToolUse(tool_use) = block {
                                tool_uses.push(tool_use.name.clone());
                            }
                        }
                        if let Some(usage) = &msg.message.usage {
                            if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64())
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
                            if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64())
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
                Err(_) => {}
            }
        }
    }

    client.disconnect().await?;

    // Check if code was changed (Edit or Write tool was used)
    let code_changed = tool_uses.iter().any(|t| t == "Edit" || t == "Write");

    // Get queue
    let queue = human_queue.lock().unwrap().clone();

    Ok((total_tokens, code_changed, queue))
}
