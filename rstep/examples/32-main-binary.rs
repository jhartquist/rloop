//! Example 32: Main Binary Implementation
//!
//! This is the near-final CLI structure for rstep - the main binary entry point.
//!
//! Wire up all components:
//! - CLI argument parsing (from example 29)
//! - Task resolution (from example 30)
//! - Dry run mode (from example 31)
//! - Full step execution (from step module)
//!
//! Exit codes:
//! - 0: Success
//! - 1: Failed (or error)
//! - 2: Blocked
//!
//! Run with:
//!   cargo run --example 32-main-binary -- --help
//!   cargo run --example 32-main-binary -- --dry-run
//!   cargo run --example 32-main-binary -- bd-123 --dry-run
//!   cargo run --example 32-main-binary -- --no-merge

use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use rstep::step::{load_config, run_step, Blocker, Config, StepResult, Task};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

// ============================================================
// EXIT CODES
// ============================================================

/// Exit code for successful step completion.
const EXIT_SUCCESS: i32 = 0;

/// Exit code for failed step or error.
const EXIT_FAILED: i32 = 1;

/// Exit code for blocked step (needs human input).
const EXIT_BLOCKED: i32 = 2;

// ============================================================
// CLI ARGUMENT DEFINITION
// ============================================================

/// rstep - Single-step task executor
///
/// Run one step of the rloop MDP. Gets a task, executes it in isolation,
/// verifies the result, and merges on success.
#[derive(Parser, Debug)]
#[command(name = "rstep")]
#[command(version, about, long_about = None)]
struct Args {
    /// Task ID to execute (e.g., bd-abc123). If omitted, gets next ready task from br.
    task_id: Option<String>,

    /// Preview execution plan without running
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Skip merge step (for parallel orchestration)
    #[arg(long)]
    no_merge: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Config file path (default: .rloop/config.toml)
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,
}

// ============================================================
// TASK RESOLUTION
// ============================================================

/// Result of attempting to resolve a task.
#[derive(Debug)]
enum TaskResolution {
    /// Successfully resolved a task
    Resolved { task: Task, source: TaskSource },
    /// br command not found (not installed)
    BrNotInstalled,
    /// No tasks available (empty ready queue)
    NoTasksAvailable,
    /// Task ID provided but task not found
    TaskNotFound(String),
    /// Error during resolution
    Error(String),
}

/// Where the task came from.
#[derive(Debug, Clone)]
enum TaskSource {
    /// Provided via CLI argument
    CliArgument,
    /// Retrieved from br ready queue
    BrReadyQueue,
    /// Mock task (for demo when br not available)
    Mock,
}

impl std::fmt::Display for TaskSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskSource::CliArgument => write!(f, "CLI argument"),
            TaskSource::BrReadyQueue => write!(f, "br ready queue"),
            TaskSource::Mock => write!(f, "mock (demo)"),
        }
    }
}

/// br's JSON output format.
#[derive(Debug, Deserialize)]
struct BrTask {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    status: String,
    priority: i32,
    issue_type: String,
    #[serde(default)]
    acceptance_criteria: Option<String>,
}

impl From<BrTask> for Task {
    fn from(br: BrTask) -> Self {
        Task {
            id: br.id,
            title: br.title,
            description: br.description,
            status: br.status,
            priority: br.priority,
            issue_type: br.issue_type,
            acceptance_criteria: br.acceptance_criteria,
        }
    }
}

/// Resolve a task based on CLI arguments.
fn resolve_task(args: &Args) -> TaskResolution {
    match &args.task_id {
        Some(id) => get_task_by_id(id),
        None => get_next_ready_task(),
    }
}

/// Get a specific task by ID from br.
fn get_task_by_id(task_id: &str) -> TaskResolution {
    let output = match Command::new("br")
        .args(["show", task_id, "--json"])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return TaskResolution::BrNotInstalled;
            }
            return TaskResolution::Error(format!("Failed to run br: {}", e));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("No such") {
            return TaskResolution::TaskNotFound(task_id.to_string());
        }
        return TaskResolution::Error(format!("br show failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<BrTask>(&stdout) {
        Ok(br_task) => TaskResolution::Resolved {
            task: br_task.into(),
            source: TaskSource::CliArgument,
        },
        Err(e) => TaskResolution::Error(format!("Failed to parse br output: {}", e)),
    }
}

/// Get the next ready task from br.
fn get_next_ready_task() -> TaskResolution {
    let output = match Command::new("br")
        .args(["ready", "--json", "--limit", "1"])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return TaskResolution::BrNotInstalled;
            }
            return TaskResolution::Error(format!("Failed to run br: {}", e));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return TaskResolution::Error(format!("br failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    if stdout.is_empty() || stdout == "[]" {
        return TaskResolution::NoTasksAvailable;
    }

    match serde_json::from_str::<Vec<BrTask>>(stdout) {
        Ok(tasks) => {
            if tasks.is_empty() {
                TaskResolution::NoTasksAvailable
            } else {
                TaskResolution::Resolved {
                    task: tasks.into_iter().next().unwrap().into(),
                    source: TaskSource::BrReadyQueue,
                }
            }
        }
        Err(e) => TaskResolution::Error(format!("Failed to parse br output: {}", e)),
    }
}

/// Create a mock task for demonstration purposes.
fn create_mock_task(task_id: Option<&str>) -> Task {
    Task {
        id: task_id.unwrap_or("bd-mock123").to_string(),
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

// ============================================================
// DRY RUN OUTPUT
// ============================================================

/// Print the dry run output for a task.
fn print_dry_run(task: &Task, source: &TaskSource, config: &Config, args: &Args) {
    let separator = "=".repeat(60);

    println!();
    println!(
        "{}",
        "DRY RUN - No changes will be made".yellow().bold()
    );
    println!("{}", separator.dimmed());

    // [TASK] section
    println!("\n{}", "[TASK]".cyan().bold());
    println!("  {} {}", "ID:".white(), task.id.cyan().bold());
    println!("  {} {}", "Title:".white(), task.title);
    println!("  {} {}", "Source:".white(), source.to_string().dimmed());
    println!("  {} {}", "Type:".white(), format_type(&task.issue_type));
    println!(
        "  {} {}",
        "Priority:".white(),
        format_priority(task.priority)
    );

    // [CONFIG] section
    println!("\n{}", "[CONFIG]".cyan().bold());
    println!("  {} {}", "Model:".white(), config.step.model.green());
    println!(
        "  {} {}",
        "Tools:".white(),
        config.step.tools.allowed.join(", ")
    );
    println!(
        "  {} {}",
        "Verification:".white(),
        config.verification.commands.join(", ")
    );

    let auto_merge = if args.no_merge {
        "false (--no-merge)".yellow().to_string()
    } else if config.worktree.auto_merge {
        "true".green().to_string()
    } else {
        "false".dimmed().to_string()
    };
    println!("  {} {}", "Auto-merge:".white(), auto_merge);

    // [EXECUTION PLAN] section
    println!("\n{}", "[EXECUTION PLAN]".cyan().bold());
    let worktree_path = format!("{}/{}", config.worktree.root, task.id);
    let branch_name = format!("rloop/{}", task.id);

    println!(
        "  {} Create worktree at {}",
        "1.".dimmed(),
        worktree_path.cyan()
    );
    println!(
        "  {} Create branch {}",
        "2.".dimmed(),
        branch_name.cyan()
    );
    println!("  {} Lock worktree", "3.".dimmed());
    println!(
        "  {} Run Claude with task prompt ({} model)",
        "4.".dimmed(),
        config.step.model.green()
    );
    println!("  {} Commit changes (if any)", "5.".dimmed());
    println!("  {} Run verification commands:", "6.".dimmed());
    for cmd in &config.verification.commands {
        println!("     {} {}", "-".dimmed(), cmd);
    }

    if args.no_merge {
        println!(
            "  {} {} (--no-merge specified)",
            "7.".dimmed(),
            "Skip merge".yellow()
        );
    } else {
        println!("  {} If success: Merge to main, close task", "7.".dimmed());
    }
    println!("  {} If failed: Reset task to open", "8.".dimmed());
    println!("  {} If blocked: Mark task blocked", "9.".dimmed());
    println!("  {} Cleanup worktree", "10.".dimmed());

    // [PROMPT PREVIEW] section
    if args.verbose {
        println!("\n{}", "[PROMPT]".cyan().bold());
        println!("{}", "-".repeat(40).dimmed());
        for line in task.to_prompt().lines() {
            println!("  {}", line);
        }
        println!("{}", "-".repeat(40).dimmed());
    }

    println!("\n{}", separator.dimmed());
    println!(
        "{}",
        "Run without --dry-run to execute".white().bold()
    );
    println!();
}

/// Format priority level with color.
fn format_priority(priority: i32) -> String {
    match priority {
        0 => "P0 (Critical)".red().bold().to_string(),
        1 => "P1 (High)".yellow().bold().to_string(),
        2 => "P2 (Medium)".white().to_string(),
        3 => "P3 (Low)".dimmed().to_string(),
        4 => "P4 (Backlog)".dimmed().to_string(),
        _ => format!("P{}", priority),
    }
}

/// Format issue type with color.
fn format_type(issue_type: &str) -> String {
    match issue_type {
        "bug" => "bug".red().to_string(),
        "feature" => "feature".green().to_string(),
        "task" => "task".blue().to_string(),
        "chore" => "chore".dimmed().to_string(),
        "epic" => "epic".magenta().to_string(),
        other => other.to_string(),
    }
}

// ============================================================
// RESULT OUTPUT
// ============================================================

/// Print the step result with colored output.
fn print_result(result: &StepResult) {
    match result {
        StepResult::Success {
            summary,
            tokens_used,
            duration_ms,
        } => {
            println!();
            println!(
                "{} {}",
                "SUCCESS".green().bold(),
                summary
            );
            if *tokens_used > 0 {
                println!(
                    "  {} tokens, {}ms",
                    tokens_used.to_string().dimmed(),
                    duration_ms.to_string().dimmed()
                );
            }
        }
        StepResult::Failed {
            reason,
            next_steps,
            tokens_used,
        } => {
            println!();
            println!(
                "{} {}",
                "FAILED".red().bold(),
                reason
            );
            if !next_steps.is_empty() {
                println!("{}", "Next steps:".dimmed());
                for step in next_steps {
                    println!("  {} {}", "-".dimmed(), step);
                }
            }
            if *tokens_used > 0 {
                println!("  {} tokens", tokens_used.to_string().dimmed());
            }
        }
        StepResult::Blocked { blocker } => {
            println!();
            println!("{}", "BLOCKED".yellow().bold());
            match blocker {
                Blocker::Question { id, text } => {
                    println!("  {} {}", "Question:".white(), text);
                    println!("  {} {}", "ID:".dimmed(), id);
                }
                Blocker::Assumption { id, needs_review } => {
                    println!("  {} Assumption needs review", "Blocker:".white());
                    println!("  {} {}", "ID:".dimmed(), id);
                    println!("  {} {}", "Needs review:".dimmed(), needs_review);
                }
                Blocker::Dependency { task_id } => {
                    println!("  {} {}", "Blocked by task:".white(), task_id);
                }
            }
        }
    }
}

// ============================================================
// MAIN
// ============================================================

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Resolve the task first
    let (task, source) = match resolve_task_with_fallback(&args) {
        Ok(result) => result,
        Err(msg) => {
            eprintln!("{} {}", "Error:".red().bold(), msg);
            std::process::exit(EXIT_FAILED);
        }
    };

    // Load config
    let config_path = args.config.clone().unwrap_or_else(|| PathBuf::from("."));
    let config = load_config(&config_path);

    // Dry run - show plan and exit
    if args.dry_run {
        print_dry_run(&task, &source, &config, &args);
        std::process::exit(EXIT_SUCCESS);
    }

    // Execute the step
    if args.verbose {
        println!(
            "{} {} ({})",
            "Executing task:".cyan().bold(),
            task.id.cyan(),
            task.title
        );
    }

    // For this example, we need a repo path - use current directory
    let repo_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match run_step(&repo_path, &task, &config).await {
        Ok(result) => {
            print_result(&result);
            match result {
                StepResult::Success { .. } => std::process::exit(EXIT_SUCCESS),
                StepResult::Failed { .. } => std::process::exit(EXIT_FAILED),
                StepResult::Blocked { .. } => std::process::exit(EXIT_BLOCKED),
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
            std::process::exit(EXIT_FAILED);
        }
    }
}

/// Resolve task with fallback to mock if br not installed.
fn resolve_task_with_fallback(args: &Args) -> Result<(Task, TaskSource), String> {
    match resolve_task(args) {
        TaskResolution::Resolved { task, source } => Ok((task, source)),

        TaskResolution::BrNotInstalled => {
            // Fall back to mock task for demo
            eprintln!(
                "{} br not installed - using mock task",
                "Note:".yellow().bold()
            );
            let task = create_mock_task(args.task_id.as_deref());
            Ok((task, TaskSource::Mock))
        }

        TaskResolution::NoTasksAvailable => {
            Err("No tasks available in ready queue".to_string())
        }

        TaskResolution::TaskNotFound(id) => {
            Err(format!("Task not found: {}", id))
        }

        TaskResolution::Error(msg) => {
            Err(msg)
        }
    }
}
