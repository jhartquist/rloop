//! Example 26: br Status Updates
//!
//! Demonstrates updating task status in beads_rust (br) during step execution.
//!
//! This example shows the full state machine for task status:
//!   open -> in_progress -> closed (success)
//!                       -> open (failure, for retry)
//!                       -> blocked (needs human input)
//!
//! Key functions:
//!   - br_mark_in_progress(task_id) - Mark task as being worked on
//!   - br_close(task_id, summary) - Mark task as completed
//!   - br_mark_open(task_id) - Reset to open (for retry after failure)
//!   - br_mark_blocked(task_id) - Mark as blocked on human input
//!
//! Run with:
//!   cargo run --example 26-br-status-updates
//!
//! Key additions from example 12:
//!   - Status update functions for br CLI
//!   - State machine visualization
//!   - Graceful handling when br not installed (simulation mode)

use anyhow::{anyhow, Result};
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tracing::{debug, info};

// ============================================================
// TYPES
// ============================================================

/// A task from beads_rust (br).
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    /// Unique task ID (e.g., "bd-abc123")
    pub id: String,

    /// Task title
    pub title: String,

    /// Detailed description (optional)
    #[serde(default)]
    pub description: Option<String>,

    /// Task status (open, in_progress, etc.)
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

/// Result of attempting to get a task from br.
#[derive(Debug)]
pub enum BrResult {
    Task(Task),
    NotInstalled,
    NoTasks,
    Error(String),
}

/// Result of a step execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepResult {
    Success { summary: String },
    Failed { reason: String },
    Blocked { blocker: String },
}

// ============================================================
// BR CLI INTEGRATION - READING TASKS
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
pub fn get_ready_task() -> BrResult {
    let output = match Command::new("br")
        .args(["ready", "--json", "--limit", "1"])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return BrResult::NotInstalled;
            }
            return BrResult::Error(format!("Failed to run br: {}", e));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a beads repository")
            || stderr.contains("no .beads directory")
            || stderr.contains("could not find")
        {
            return BrResult::Error("Not in a beads repository".to_string());
        }
        return BrResult::Error(format!("br failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    if stdout.is_empty() || stdout == "[]" {
        return BrResult::NoTasks;
    }

    match serde_json::from_str::<Vec<Task>>(stdout) {
        Ok(tasks) => {
            if tasks.is_empty() {
                BrResult::NoTasks
            } else {
                BrResult::Task(tasks.into_iter().next().unwrap())
            }
        }
        Err(e) => BrResult::Error(format!("Failed to parse br output: {}\nOutput: {}", e, stdout)),
    }
}

// ============================================================
// BR CLI INTEGRATION - STATUS UPDATES
// ============================================================

/// Mark a task as in_progress
///
/// State transition: open -> in_progress
pub fn br_mark_in_progress(task_id: &str, simulate: bool) -> Result<()> {
    let cmd = format!("br update {} --status in_progress", task_id);

    if simulate {
        println!(
            "  {} {}",
            "[SIMULATE]".yellow().bold(),
            cmd.white().dimmed()
        );
        return Ok(());
    }

    let status = Command::new("br")
        .args(["update", task_id, "--status", "in_progress"])
        .status()?;

    if !status.success() {
        return Err(anyhow!("Failed to mark task {} as in_progress", task_id));
    }

    Ok(())
}

/// Close a task with a summary
///
/// State transition: in_progress -> closed
pub fn br_close(task_id: &str, summary: &str, simulate: bool) -> Result<()> {
    let cmd = format!("br close {} --summary \"{}\"", task_id, summary);

    if simulate {
        println!(
            "  {} {}",
            "[SIMULATE]".yellow().bold(),
            cmd.white().dimmed()
        );
        return Ok(());
    }

    let status = Command::new("br")
        .args(["close", task_id, "--summary", summary])
        .status()?;

    if !status.success() {
        return Err(anyhow!("Failed to close task {}", task_id));
    }

    Ok(())
}

/// Mark a task as open (for retry after failure)
///
/// State transition: in_progress -> open
pub fn br_mark_open(task_id: &str, simulate: bool) -> Result<()> {
    let cmd = format!("br update {} --status open", task_id);

    if simulate {
        println!(
            "  {} {}",
            "[SIMULATE]".yellow().bold(),
            cmd.white().dimmed()
        );
        return Ok(());
    }

    let status = Command::new("br")
        .args(["update", task_id, "--status", "open"])
        .status()?;

    if !status.success() {
        return Err(anyhow!("Failed to mark task {} as open", task_id));
    }

    Ok(())
}

/// Mark a task as blocked
///
/// State transition: in_progress -> blocked
pub fn br_mark_blocked(task_id: &str, simulate: bool) -> Result<()> {
    let cmd = format!("br update {} --status blocked", task_id);

    if simulate {
        println!(
            "  {} {}",
            "[SIMULATE]".yellow().bold(),
            cmd.white().dimmed()
        );
        return Ok(());
    }

    let status = Command::new("br")
        .args(["update", task_id, "--status", "blocked"])
        .status()?;

    if !status.success() {
        return Err(anyhow!("Failed to mark task {} as blocked", task_id));
    }

    Ok(())
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

fn print_state_machine() {
    println!("\n{}", "Task Status State Machine:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!();
    println!("  {}", "+-------+".cyan());
    println!("  {}", "| open  |".cyan());
    println!("  {}", "+---+---+".cyan());
    println!("      |");
    println!("      | br_mark_in_progress()");
    println!("      v");
    println!("  {}", "+-------------+".yellow());
    println!("  {}", "| in_progress |".yellow());
    println!("  {}", "+------+------+".yellow());
    println!("         |");
    println!("         +-------------------------------+----------------------+");
    println!("         |                               |                      |");
    println!("         | br_close()                    | br_mark_open()       | br_mark_blocked()");
    println!("         v                               v                      v");
    println!(
        "   {}                        {}                   {}",
        "+--------+".green(),
        "+-------+".red(),
        "+---------+".magenta()
    );
    println!(
        "   {}                        {}                   {}",
        "| closed |".green(),
        "| open  |".red(),
        "| blocked |".magenta()
    );
    println!(
        "   {}                        {}                   {}",
        "+--------+".green(),
        "+-------+".red(),
        "+---------+".magenta()
    );
    println!(
        "   {} SUCCESS                 {} RETRY              {} HUMAN INPUT",
        "(done)".green().dimmed(),
        "(fail)".red().dimmed(),
        "(needs)".magenta().dimmed()
    );
    println!();
    println!("{}", "-".repeat(60).dimmed());
}

fn print_status_transition(from: &str, to: &str, via: &str) {
    let from_colored = match from {
        "open" => from.cyan(),
        "in_progress" => from.yellow(),
        "closed" => from.green(),
        "blocked" => from.magenta(),
        _ => from.white(),
    };
    let to_colored = match to {
        "open" => to.cyan(),
        "in_progress" => to.yellow(),
        "closed" => to.green(),
        "blocked" => to.magenta(),
        _ => to.white(),
    };

    println!(
        "\n  {} {} {} {} {}",
        "Status:".dimmed(),
        from_colored,
        "->".white().bold(),
        to_colored,
        format!("({})", via).dimmed()
    );
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
        "|                   BR STATUS UPDATES                              |"
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
        "|  Demonstrates task status transitions during step execution.     |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+------------------------------------------------------------------+"
            .cyan()
            .bold()
    );

    // Show the state machine
    print_state_machine();

    // Check if br is installed
    let simulate = !br_is_installed();
    if simulate {
        println!(
            "\n{} br CLI not installed - running in {} mode",
            "NOTE:".yellow().bold(),
            "SIMULATION".yellow().bold()
        );
        println!(
            "{}",
            "Commands will be shown but not executed.".dimmed()
        );
    } else {
        println!(
            "\n{} br CLI detected - commands will be executed",
            "NOTE:".green().bold()
        );
    }

    // ============================================================
    // STEP 1: GET TASK FROM BR (or use mock)
    // ============================================================

    println!("\n{}", "1. Getting task from br...".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    let task = if simulate {
        // Use a mock task for demonstration
        println!(
            "  {} Using mock task for demonstration",
            "[SIMULATE]".yellow().bold()
        );
        Task {
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
    } else {
        match get_ready_task() {
            BrResult::Task(t) => t,
            BrResult::NoTasks => {
                println!(
                    "  {} No tasks available in br ready queue",
                    "INFO".yellow().bold()
                );
                println!("  Using mock task for demonstration...");
                Task {
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
            BrResult::Error(e) => {
                println!(
                    "  {} {} - using mock task",
                    "WARNING".yellow().bold(),
                    e
                );
                Task {
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
            BrResult::NotInstalled => unreachable!(), // Already handled by simulate check
        }
    };

    println!("\n  {} {}", "Task ID:".dimmed(), task.id.cyan().bold());
    println!("  {} {}", "Title:".dimmed(), task.title.white().bold());
    println!(
        "  {} {}",
        "Current status:".dimmed(),
        task.status.cyan()
    );

    // ============================================================
    // STEP 2: MARK AS IN_PROGRESS
    // ============================================================

    println!("\n{}", "2. Marking task as in_progress...".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    br_mark_in_progress(&task.id, simulate)?;
    print_status_transition("open", "in_progress", "br_mark_in_progress");

    // ============================================================
    // STEP 3: EXECUTE THE STEP (Claude fixes the bug)
    // ============================================================

    println!("\n{}", "3. Executing step...".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    // Set up working directory
    let temp = tempdir()?;
    info!("{}", "Setting up working directory...".cyan().bold());

    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    debug!("Template source: {:?}", template_dir);
    debug!("Working directory: {:?}", temp.path());

    copy_dir_all(&template_dir, temp.path())?;
    info!(
        "  {} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    // Show the buggy code
    let lib_path = temp.path().join("src/lib.rs");
    println!("\n  {}", "Current code (buggy):".yellow().bold());
    let original_code = fs::read_to_string(&lib_path)?;
    for line in original_code.lines().take(5) {
        println!("    {}", line.dimmed());
    }
    println!("    {}", "...".dimmed());

    // Configure Claude
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec![
            "Read".to_string(),
            "Write".to_string(),
            "Edit".to_string(),
            "Bash".to_string(),
        ])
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // Send the task prompt
    let prompt = format!(
        r#"{}

Fix this bug. Read src/lib.rs, fix the issue, and verify with cargo test.

When done, output your result as JSON in exactly this format:
```json
{{"result": "Success", "summary": "Fixed the bug by changing X to Y"}}
```

Or if it fails:
```json
{{"result": "Failed", "reason": "Description of what went wrong"}}
```"#,
        task.to_prompt()
    );

    println!(
        "\n  {} {}",
        "Sending to Claude:".yellow().bold(),
        task.title.white()
    );
    client.query(&prompt).await?;

    // Receive response
    let mut response_text = String::new();
    let mut total_tokens: u64 = 0;

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => match &message {
                    Message::Assistant(msg) => {
                        for block in &msg.message.content {
                            if let claude_agent_sdk_rs::ContentBlock::Text(text_block) = block {
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

    // Check if the code was fixed
    let fixed_code = fs::read_to_string(&lib_path)?;
    let code_fixed = fixed_code.contains("a + b") && !fixed_code.contains("a - b");

    // Run cargo test to verify
    let test_output = Command::new("cargo")
        .args(["test"])
        .current_dir(temp.path())
        .output()?;
    let tests_pass = test_output.status.success();

    println!(
        "\n  {} {}",
        "Code fixed:".dimmed(),
        if code_fixed {
            "Yes".green()
        } else {
            "No".red()
        }
    );
    println!(
        "  {} {}",
        "Tests pass:".dimmed(),
        if tests_pass {
            "Yes".green()
        } else {
            "No".red()
        }
    );
    println!("  {} {}", "Tokens used:".dimmed(), total_tokens);

    // ============================================================
    // STEP 4: UPDATE STATUS BASED ON RESULT
    // ============================================================

    println!("\n{}", "4. Updating task status based on result...".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    let step_result = if code_fixed && tests_pass {
        // SUCCESS: close the task
        let summary = "Fixed add function: changed a - b to a + b";
        br_close(&task.id, summary, simulate)?;
        print_status_transition("in_progress", "closed", "br_close");
        StepResult::Success {
            summary: summary.to_string(),
        }
    } else if !code_fixed {
        // FAILED: reset to open for retry
        br_mark_open(&task.id, simulate)?;
        print_status_transition("in_progress", "open", "br_mark_open");
        StepResult::Failed {
            reason: "Claude did not fix the bug".to_string(),
        }
    } else {
        // Tests failed: could be blocked or failed
        br_mark_open(&task.id, simulate)?;
        print_status_transition("in_progress", "open", "br_mark_open");
        StepResult::Failed {
            reason: "Tests did not pass after changes".to_string(),
        }
    };

    // ============================================================
    // STEP 5: DISPLAY FINAL RESULT
    // ============================================================

    println!("\n{}", "5. Step Result:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    match &step_result {
        StepResult::Success { summary } => {
            println!(
                "  {} {}",
                "Status:".dimmed(),
                "SUCCESS".green().bold()
            );
            println!("  {} {}", "Summary:".dimmed(), summary);
            println!(
                "\n  {} Task will be {} in br",
                "->".green(),
                "closed".green().bold()
            );
        }
        StepResult::Failed { reason } => {
            println!(
                "  {} {}",
                "Status:".dimmed(),
                "FAILED".red().bold()
            );
            println!("  {} {}", "Reason:".dimmed(), reason);
            println!(
                "\n  {} Task reset to {} for retry",
                "->".red(),
                "open".cyan().bold()
            );
        }
        StepResult::Blocked { blocker } => {
            println!(
                "  {} {}",
                "Status:".dimmed(),
                "BLOCKED".magenta().bold()
            );
            println!("  {} {}", "Blocker:".dimmed(), blocker);
            println!(
                "\n  {} Task marked {} pending human input",
                "->".magenta(),
                "blocked".magenta().bold()
            );
        }
    }

    // Show the JSON
    println!("\n{}", "Step Result (JSON):".blue().bold());
    println!("{}", serde_json::to_string_pretty(&step_result)?);

    // ============================================================
    // DEMONSTRATE BLOCKED SCENARIO
    // ============================================================

    println!("\n{}", "--- BONUS: Demonstrating BLOCKED scenario ---".magenta().bold());
    println!("{}", "-".repeat(60).dimmed());

    // Show what would happen if the task was blocked
    println!(
        "\n  {} If the task required human input:",
        "Example:".yellow().bold()
    );
    br_mark_blocked("bd-example456", true)?;
    print_status_transition("in_progress", "blocked", "br_mark_blocked");

    let blocked_result = StepResult::Blocked {
        blocker: "Need clarification on error handling requirements".to_string(),
    };
    println!("\n{}", serde_json::to_string_pretty(&blocked_result)?);

    // Summary
    println!("\n{}", "=".repeat(60).cyan().bold());
    println!(
        "{}",
        "                    STATUS UPDATE SUMMARY                    ".cyan().bold()
    );
    println!("{}", "=".repeat(60).cyan().bold());

    println!("\n  {} br commands for status management:", "Key".white().bold());
    println!(
        "    {} {}",
        "Start work:".dimmed(),
        "br update <id> --status in_progress".white()
    );
    println!(
        "    {} {}",
        "On success:".dimmed(),
        "br close <id> --summary \"...\"".white()
    );
    println!(
        "    {} {}",
        "On failure:".dimmed(),
        "br update <id> --status open".white()
    );
    println!(
        "    {} {}",
        "On blocked:".dimmed(),
        "br update <id> --status blocked".white()
    );

    println!(
        "\n{}",
        "Status updates enable task orchestration and retry logic!".yellow().dimmed()
    );

    Ok(())
}
