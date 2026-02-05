//! Example 28: Full Step Flow
//!
//! The complete step executor combining all previous examples (00-27).
//!
//! This is the culmination of rstep - a single, complete step execution
//! implementing the full flow from RSTEP.md:
//!
//! 1. [CONFIG]   Load configuration from .rloop/config.toml
//! 2. [TASK]     Get task from br (or use mock if br unavailable)
//! 3. [BLOCKER]  Check for blockers (pending human queue items)
//! 4. [WORKTREE] Create worktree with task branch
//! 5. [LOCK]     Lock worktree during work
//! 6. [EXECUTE]  Run Claude with controlled environment + human queue tools
//! 7. [COMMIT]   Commit changes in worktree
//! 8. [VERIFY]   Run verification (cargo check, cargo test)
//! 9. [MERGE]    On success: merge to main, update br status to closed
//!               On failure: update br status to open (for retry)
//!               On blocked: update br status to blocked
//! 10. [CLEANUP] Remove worktree, delete branch
//! 11. [LOG]     Log session (summary)
//! 12. [RESULT]  Return StepResult
//!
//! Run with:
//!   cargo run --example 28-full-step-flow
//!
//! This example uses library modules:
//!   - rstep::git for worktree operations
//!   - rstep::human_queue for human-in-the-loop tools

use anyhow::{Context, Result};
use chrono::Utc;
use claude_agent_sdk_rs::{
    create_sdk_mcp_server, ClaudeAgentOptions, ClaudeClient, ContentBlock, McpServerConfig,
    McpServers, Message, PermissionMode,
};
use colored::Colorize;
use futures::StreamExt;
use rstep::git::{
    cleanup_worktree, commit_changes, create_worktree, get_log, init_repo, lock_worktree,
    merge_to_main, unlock_worktree,
};
use rstep::human_queue::{create_human_queue_tools, HumanQueue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
// tracing available if needed

// ============================================================
// CONFIG TYPES (from example 27)
// ============================================================

/// Root configuration structure.
#[derive(Debug, Deserialize, Clone)]
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

#[derive(Debug, Deserialize, Clone)]
pub struct StepConfig {
    #[serde(default = "default_model")]
    pub model: String,

    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

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

#[derive(Debug, Deserialize, Clone)]
pub struct ToolsConfig {
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

#[derive(Debug, Deserialize, Clone)]
pub struct VerificationConfig {
    #[serde(default = "default_verification_commands")]
    pub commands: Vec<String>,

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

#[derive(Debug, Deserialize, Clone)]
pub struct WorktreeConfig {
    #[serde(default = "default_worktree_root")]
    pub root: String,

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

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_session_dir")]
    pub session_dir: String,

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
// TASK TYPES (from example 26)
// ============================================================

/// A task from beads_rust (br).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub status: String,
    pub priority: i32,
    pub issue_type: String,
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

    /// Create a mock task for demonstration when br is unavailable.
    pub fn mock() -> Self {
        Self {
            id: "bd-mock123".to_string(),
            title: "Fix the bug in add function".to_string(),
            description: Some("The add function returns a-b instead of a+b".to_string()),
            status: "open".to_string(),
            priority: 2,
            issue_type: "bug".to_string(),
            acceptance_criteria: Some(
                "- add(2, 3) should return 5\n- add(-1, 1) should return 0".to_string(),
            ),
        }
    }
}

// ============================================================
// STEP RESULT TYPES
// ============================================================

/// Blocker types that prevent step completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Blocker {
    Question {
        id: String,
        text: String,
    },
    Assumption {
        id: String,
        needs_review: bool,
    },
    Dependency {
        task_id: String,
    },
}

/// Result of a step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepResult {
    Success {
        summary: String,
        tokens_used: u64,
        duration_ms: u64,
    },
    Failed {
        reason: String,
        next_steps: Vec<String>,
        tokens_used: u64,
    },
    Blocked {
        blocker: Blocker,
    },
}

// ============================================================
// BR CLI INTEGRATION
// ============================================================

/// Check if br CLI is installed
fn br_is_installed() -> bool {
    Command::new("br")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the next ready task from br.
fn get_ready_task() -> Option<Task> {
    let output = Command::new("br")
        .args(["ready", "--json", "--limit", "1"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    if stdout.is_empty() || stdout == "[]" {
        return None;
    }

    serde_json::from_str::<Vec<Task>>(stdout)
        .ok()
        .and_then(|tasks| tasks.into_iter().next())
}

/// Update task status in br (simulates if br not available)
fn br_update_status(task_id: &str, status: &str, simulate: bool) -> Result<()> {
    let cmd = format!("br update {} --status {}", task_id, status);

    if simulate {
        println!(
            "    {} {}",
            "[SIMULATE]".yellow().bold(),
            cmd.white().dimmed()
        );
        return Ok(());
    }

    let result = Command::new("br")
        .args(["update", task_id, "--status", status])
        .status();

    match result {
        Ok(s) if s.success() => Ok(()),
        _ => {
            println!(
                "    {} Failed to update status (br may not be installed)",
                "WARNING:".yellow()
            );
            Ok(())
        }
    }
}

/// Close task in br (simulates if br not available)
fn br_close(task_id: &str, summary: &str, simulate: bool) -> Result<()> {
    let cmd = format!("br close {} --summary \"{}\"", task_id, summary);

    if simulate {
        println!(
            "    {} {}",
            "[SIMULATE]".yellow().bold(),
            cmd.white().dimmed()
        );
        return Ok(());
    }

    let result = Command::new("br")
        .args(["close", task_id, "--summary", summary])
        .status();

    match result {
        Ok(s) if s.success() => Ok(()),
        _ => {
            println!(
                "    {} Failed to close task (br may not be installed)",
                "WARNING:".yellow()
            );
            Ok(())
        }
    }
}

// ============================================================
// HELPERS
// ============================================================

/// Recursively copy a directory and all its contents
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Run verification commands in the given directory
fn run_verification(path: &Path, commands: &[String], fail_fast: bool) -> Result<(bool, String)> {
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

/// Load config from .rloop/config.toml
fn load_config(base_dir: &Path) -> Config {
    let config_path = base_dir.join(".rloop/config.toml");

    if !config_path.exists() {
        return Config::default();
    }

    match fs::read_to_string(&config_path) {
        Ok(contents) => toml::from_str::<Config>(&contents).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// Print a phase banner
fn print_phase(phase: &str, description: &str) {
    println!();
    println!("{}", "=".repeat(70).cyan());
    println!(
        "  {} {} - {}",
        format!("[{}]", phase).cyan().bold(),
        phase.yellow().bold(),
        description.white()
    );
    println!("{}", "=".repeat(70).cyan());
}

/// Print a sub-step
fn print_step(step: &str) {
    println!("  {} {}", "->".green(), step);
}

/// Print a key-value pair
fn print_kv(key: &str, value: &str) {
    println!("    {} {}", format!("{}:", key).dimmed(), value.white());
}

// ============================================================
// MAIN STEP EXECUTION
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

    let start_time = Utc::now();

    println!(
        "\n{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                        FULL STEP FLOW                                |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                                      |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  The complete step executor - combining examples 00-27               |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );

    // Track whether we're simulating (br not installed)
    let simulate_br = !br_is_installed();
    if simulate_br {
        println!(
            "\n{} br CLI not installed - running in {} mode",
            "NOTE:".yellow().bold(),
            "SIMULATION".yellow().bold()
        );
    }

    // ============================================================
    // PHASE 1: CONFIG
    // ============================================================

    print_phase("CONFIG", "Load configuration from .rloop/config.toml");

    // For this example, we'll use a temp directory as our "repo"
    let temp = tempdir()?;
    let repo_path = temp.path().to_path_buf();

    // Copy template to temp directory
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    copy_dir_all(&template_dir, &repo_path)?;
    print_step("Copied bug-adder template to temp directory");

    // Initialize git repo
    init_repo(&repo_path)?;
    print_step("Initialized git repository");

    // Load config (will use defaults since no config file in temp)
    let config = load_config(&repo_path);
    print_kv("Model", &config.step.model);
    print_kv("Max turns", &config.step.max_turns.to_string());
    print_kv("Verification", &config.verification.commands.join(", "));
    print_kv("Auto merge", &config.worktree.auto_merge.to_string());

    // ============================================================
    // PHASE 2: TASK
    // ============================================================

    print_phase("TASK", "Get task from br (or use mock if unavailable)");

    let task = if simulate_br {
        print_step("br not installed - using mock task");
        Task::mock()
    } else {
        match get_ready_task() {
            Some(t) => {
                print_step("Got task from br ready queue");
                t
            }
            None => {
                print_step("No tasks in br ready queue - using mock task");
                Task::mock()
            }
        }
    };

    print_kv("Task ID", &task.id);
    print_kv("Title", &task.title);
    print_kv("Status", &task.status);

    // Mark task as in_progress
    br_update_status(&task.id, "in_progress", simulate_br)?;
    print_step(&format!(
        "Marked task {} as in_progress",
        task.id.cyan()
    ));

    // ============================================================
    // PHASE 3: BLOCKER CHECK
    // ============================================================

    print_phase("BLOCKER", "Check for pending human queue items");

    // In a real implementation, we'd check hq for blocking items
    // For this example, we'll assume no blockers
    print_step("No blocking items found - proceeding with execution");

    // ============================================================
    // PHASE 4: WORKTREE
    // ============================================================

    print_phase("WORKTREE", "Create worktree with task branch");

    let worktree_path = create_worktree(&repo_path, &task.id)?;
    print_kv("Worktree path", &worktree_path.display().to_string());
    print_kv("Branch", &format!("rloop/{}", task.id));

    // ============================================================
    // PHASE 5: LOCK
    // ============================================================

    print_phase("LOCK", "Lock worktree during work");

    lock_worktree(&worktree_path, "rstep active")?;
    print_step("Worktree locked with reason: rstep active");

    // ============================================================
    // PHASE 6: EXECUTE
    // ============================================================

    print_phase("EXECUTE", "Run Claude with controlled environment");

    // Create human queue for collecting questions/assumptions/tasks
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
        .cwd(&worktree_path)
        .build();

    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // Build the prompt
    let prompt = format!(
        r#"{}

Fix this bug in the code. Read the source file, identify the issue, and fix it.

After making changes, verify with `cargo test`.

You have access to human queue tools if needed:
- mcp__rstep-hq__ask_question - Ask if requirements are unclear
- mcp__rstep-hq__log_assumption - Log decisions for review
- mcp__rstep-hq__propose_task - Suggest follow-up work

When done, summarize what you changed."#,
        task.to_prompt()
    );

    print_kv("Model", &config.step.model);
    print_kv("Tools", &config.step.tools.allowed.join(", "));
    print_step("Sending task to Claude...");

    client.query(&prompt).await?;

    // Receive response
    let mut total_tokens: u64 = 0;
    let mut response_text = String::new();
    let mut tool_uses: Vec<String> = Vec::new();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => match &message {
                    Message::Assistant(msg) => {
                        for block in &msg.message.content {
                            match block {
                                ContentBlock::Text(text_block) => {
                                    response_text.push_str(&text_block.text);
                                }
                                ContentBlock::ToolUse(tool_use) => {
                                    tool_uses.push(tool_use.name.clone());
                                }
                                _ => {}
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
                Err(e) => {
                    eprintln!("    {} {}", "Error:".red().bold(), e);
                }
            }
        }
    }

    client.disconnect().await?;

    // Show tool usage summary
    if !tool_uses.is_empty() {
        print_step(&format!("Tools used: {}", tool_uses.join(", ")));
    }
    print_kv("Tokens used", &total_tokens.to_string());

    // Check human queue for blocking items
    let queue = human_queue.lock().unwrap().clone();
    if queue.has_blocking_items() {
        // BLOCKED: Return early with blocker
        print_step("Blocking items in human queue!");

        unlock_worktree(&worktree_path)?;
        let _ = cleanup_worktree(&repo_path, &task.id);
        br_update_status(&task.id, "blocked", simulate_br)?;

        let blocker = if let Some(q) = queue.questions.iter().find(|q| q.blocking) {
            Blocker::Question {
                id: format!("hq-q-{}", Utc::now().timestamp()),
                text: q.question.clone(),
            }
        } else {
            // Must be an assumption needing review
            Blocker::Assumption {
                id: format!("hq-a-{}", Utc::now().timestamp()),
                needs_review: true,
            }
        };

        let result = StepResult::Blocked { blocker };
        print_final_result(&result);
        return Ok(());
    }

    // Check if code was fixed
    let lib_path = worktree_path.join("src/lib.rs");
    let code = fs::read_to_string(&lib_path)?;
    let code_fixed = code.contains("a + b") && !code.contains("a - b");

    print_kv(
        "Code fixed",
        if code_fixed { "YES" } else { "NO" },
    );

    // ============================================================
    // PHASE 7: COMMIT
    // ============================================================

    print_phase("COMMIT", "Commit changes in worktree");

    let commit_result = if code_fixed {
        match commit_changes(&worktree_path, "fix: correct add function to use + instead of -") {
            Ok(hash) => {
                print_step(&format!("Committed: {}", &hash[..12].cyan()));
                Some(hash)
            }
            Err(e) => {
                print_step(&format!("No changes to commit: {}", e));
                None
            }
        }
    } else {
        print_step("Code not fixed - skipping commit");
        None
    };

    // ============================================================
    // PHASE 8: VERIFY
    // ============================================================

    print_phase("VERIFY", "Run verification commands");

    let (verification_passed, _verification_output) = run_verification(
        &worktree_path,
        &config.verification.commands,
        config.verification.fail_fast,
    )?;

    let verification_status = if verification_passed {
        "PASSED".green().to_string()
    } else {
        "FAILED".red().to_string()
    };
    print_kv("Verification", &verification_status);

    // ============================================================
    // PHASE 9: MERGE OR UPDATE STATUS
    // ============================================================

    let end_time = Utc::now();
    let duration_ms = (end_time - start_time).num_milliseconds() as u64;

    let step_result = if code_fixed && verification_passed && commit_result.is_some() {
        print_phase("MERGE", "Merge to main and close task");

        // Unlock worktree before merge
        unlock_worktree(&worktree_path)?;
        print_step("Unlocked worktree");

        // Merge to main
        let summary = "fix add function";
        merge_to_main(&repo_path, &task.id, summary)?;
        print_step(&format!("Merged to main: task: {}", summary.cyan()));

        // Close task in br
        br_close(&task.id, summary, simulate_br)?;
        print_step(&format!("Closed task {} in br", task.id.cyan()));

        // Show git log
        println!("\n  {}", "Git log (main):".yellow().bold());
        let log = get_log(&repo_path, 3)?;
        for line in log.lines() {
            println!("    {}", line);
        }

        StepResult::Success {
            summary: format!("Fixed add function: changed a - b to a + b"),
            tokens_used: total_tokens,
            duration_ms,
        }
    } else if !code_fixed {
        print_phase("FAILURE", "Code not fixed - resetting task");

        unlock_worktree(&worktree_path)?;
        br_update_status(&task.id, "open", simulate_br)?;
        print_step(&format!("Reset task {} to open for retry", task.id.cyan()));

        StepResult::Failed {
            reason: "Claude did not fix the bug in the code".to_string(),
            next_steps: vec![
                "Review the prompt for clarity".to_string(),
                "Try a different model".to_string(),
                "Provide more context".to_string(),
            ],
            tokens_used: total_tokens,
        }
    } else {
        print_phase("FAILURE", "Verification failed - resetting task");

        unlock_worktree(&worktree_path)?;
        br_update_status(&task.id, "open", simulate_br)?;
        print_step(&format!("Reset task {} to open for retry", task.id.cyan()));

        StepResult::Failed {
            reason: "Verification failed after changes".to_string(),
            next_steps: vec![
                "Check verification output".to_string(),
                "Review the changes made".to_string(),
            ],
            tokens_used: total_tokens,
        }
    };

    // ============================================================
    // PHASE 10: CLEANUP
    // ============================================================

    print_phase("CLEANUP", "Remove worktree and delete branch");

    let (wt_removed, br_deleted) = cleanup_worktree(&repo_path, &task.id)?;
    print_kv(
        "Worktree removed",
        if wt_removed { "YES" } else { "NO" },
    );
    print_kv(
        "Branch deleted",
        if br_deleted { "YES" } else { "NO" },
    );

    // ============================================================
    // PHASE 11: LOG
    // ============================================================

    print_phase("LOG", "Session summary");

    print_kv("Task ID", &task.id);
    print_kv("Duration", &format!("{}ms", duration_ms));
    print_kv("Tokens", &total_tokens.to_string());
    print_kv(
        "Human queue items",
        &queue.total_items().to_string(),
    );

    // ============================================================
    // PHASE 12: RESULT
    // ============================================================

    print_phase("RESULT", "Final step result");

    print_final_result(&step_result);

    Ok(())
}

/// Print the final step result
fn print_final_result(result: &StepResult) {
    println!();
    match result {
        StepResult::Success {
            summary,
            tokens_used,
            duration_ms,
        } => {
            println!(
                "  {} {}",
                "Status:".white().bold(),
                "SUCCESS".green().bold()
            );
            println!("  {} {}", "Summary:".dimmed(), summary);
            println!("  {} {}", "Tokens:".dimmed(), tokens_used);
            println!("  {} {}ms", "Duration:".dimmed(), duration_ms);
        }
        StepResult::Failed {
            reason,
            next_steps,
            tokens_used,
        } => {
            println!(
                "  {} {}",
                "Status:".white().bold(),
                "FAILED".red().bold()
            );
            println!("  {} {}", "Reason:".dimmed(), reason);
            println!("  {} {}", "Tokens:".dimmed(), tokens_used);
            if !next_steps.is_empty() {
                println!("  {}", "Next steps:".dimmed());
                for step in next_steps {
                    println!("    {} {}", "-".dimmed(), step);
                }
            }
        }
        StepResult::Blocked { blocker } => {
            println!(
                "  {} {}",
                "Status:".white().bold(),
                "BLOCKED".yellow().bold()
            );
            match blocker {
                Blocker::Question { id, text } => {
                    println!("  {} Question", "Blocker:".dimmed());
                    println!("  {} {}", "ID:".dimmed(), id);
                    println!("  {} {}", "Text:".dimmed(), text);
                }
                Blocker::Assumption { id, needs_review } => {
                    println!("  {} Assumption needs review", "Blocker:".dimmed());
                    println!("  {} {}", "ID:".dimmed(), id);
                    println!("  {} {}", "Needs review:".dimmed(), needs_review);
                }
                Blocker::Dependency { task_id } => {
                    println!("  {} Dependency", "Blocker:".dimmed());
                    println!("  {} {}", "Blocked by:".dimmed(), task_id);
                }
            }
        }
    }

    // Print JSON
    println!("\n  {}", "JSON:".yellow().bold());
    println!(
        "{}",
        serde_json::to_string_pretty(result)
            .unwrap_or_default()
            .lines()
            .map(|l| format!("  {}", l))
            .collect::<Vec<_>>()
            .join("\n")
    );

    println!();
    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                     FULL STEP FLOW COMPLETE                          |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
}
