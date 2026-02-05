//! Example 30: Task Resolution
//!
//! Demonstrates resolving a task from either CLI argument or br.
//!
//! This example shows how rstep determines which task to execute:
//! 1. If task_id provided via CLI: use that specific task
//! 2. If no task_id: get next ready task from br
//!
//! Run with:
//!   cargo run --example 30-task-resolution
//!   cargo run --example 30-task-resolution -- bd-123
//!   cargo run --example 30-task-resolution -- -v

use anyhow::{anyhow, Result};
use clap::Parser;
use colored::Colorize;
use rstep::step::Task;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

// ============================================================
// CLI ARGUMENT DEFINITION (from example 29)
// ============================================================

/// rstep - Single-step task executor
#[derive(Parser, Debug)]
#[command(name = "rstep")]
#[command(version, about, long_about = None)]
pub struct Args {
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
pub enum TaskResolution {
    /// Successfully resolved a task
    Resolved {
        task: Task,
        source: TaskSource,
    },
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
pub enum TaskSource {
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

/// br's JSON output format (subset of fields we need).
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
///
/// Resolution strategy:
/// 1. If task_id provided: fetch that specific task from br
/// 2. If no task_id: get next ready task from br
pub fn resolve_task(args: &Args) -> TaskResolution {
    match &args.task_id {
        Some(id) => {
            // User provided task ID - fetch from br
            get_task_by_id(id)
        }
        None => {
            // No task ID - get next ready task from br
            get_next_ready_task()
        }
    }
}

/// Get a specific task by ID from br.
fn get_task_by_id(task_id: &str) -> TaskResolution {
    // Try to run br show <task_id> --json
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
        // Check for "not found" type errors
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
    // Try to run br ready --json --limit 1
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
        if stderr.contains("not a beads repository")
            || stderr.contains("no .beads directory")
            || stderr.contains("could not find")
        {
            return TaskResolution::Error("Not in a beads repository".to_string());
        }
        return TaskResolution::Error(format!("br failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    // Empty output or empty array means no tasks
    if stdout.is_empty() || stdout == "[]" {
        return TaskResolution::NoTasksAvailable;
    }

    // br ready outputs an array
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
        Err(e) => TaskResolution::Error(format!("Failed to parse br output: {}\nOutput: {}", e, stdout)),
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
        acceptance_criteria: Some("- add(2, 3) should return 5\n- add(-1, 1) should return 0".to_string()),
    }
}

// ============================================================
// DISPLAY HELPERS
// ============================================================

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

fn print_task(task: &Task, source: &TaskSource) {
    println!("\n{}", "Resolved Task:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "Source:".dimmed(),
        source.to_string().cyan()
    );
    println!("  {} {}", "ID:".dimmed(), task.id.cyan().bold());
    println!("  {} {}", "Title:".dimmed(), task.title.white().bold());
    println!("  {} {}", "Type:".dimmed(), format_type(&task.issue_type));
    println!("  {} {}", "Priority:".dimmed(), format_priority(task.priority));
    println!("  {} {}", "Status:".dimmed(), task.status);

    if let Some(desc) = &task.description {
        println!("\n  {}", "Description:".dimmed());
        for line in desc.lines() {
            println!("    {}", line);
        }
    }

    if let Some(ac) = &task.acceptance_criteria {
        println!("\n  {}", "Acceptance Criteria:".dimmed());
        for line in ac.lines() {
            println!("    {}", line);
        }
    }

    println!("{}", "-".repeat(60).dimmed());
}

// ============================================================
// MAIN
// ============================================================

fn main() -> Result<()> {
    let args = Args::parse();

    println!();
    println!(
        "{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                        TASK RESOLUTION                               |"
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
        "|  Resolve task from CLI argument or br ready queue                    |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );

    // Show what we're trying to resolve
    println!("\n{}", "Resolution Strategy:".yellow().bold());
    println!("{}", "-".repeat(50));
    match &args.task_id {
        Some(id) => {
            println!(
                "  {} Task ID provided: {}",
                "->".cyan(),
                id.cyan().bold()
            );
            println!("     Will fetch specific task from br");
        }
        None => {
            println!("  {} No task ID provided", "->".cyan());
            println!("     Will get next ready task from br");
        }
    }
    println!("{}", "-".repeat(50));

    // Attempt resolution
    println!("\n{}", "Resolving task...".yellow());

    let resolution = resolve_task(&args);

    match resolution {
        TaskResolution::Resolved { task, source } => {
            println!(
                "\n{} Task resolved!",
                "SUCCESS".green().bold()
            );
            print_task(&task, &source);

            // Show what would happen next
            println!("\n{}", "Next Steps (if not --dry-run):".yellow().bold());
            println!("  {} Create worktree for task {}", "1.".dimmed(), task.id.cyan());
            println!("  {} Execute Claude with task prompt", "2.".dimmed());
            println!("  {} Run verification", "3.".dimmed());
            if args.no_merge {
                println!(
                    "  {} {} (--no-merge)",
                    "4.".dimmed(),
                    "Skip merge".yellow()
                );
            } else {
                println!("  {} Merge to main on success", "4.".dimmed());
            }
            println!("  {} Cleanup worktree", "5.".dimmed());

            if args.verbose {
                println!("\n{}", "Debug - Generated Prompt:".yellow().bold());
                println!("{}", "-".repeat(60).dimmed());
                println!("{}", task.to_prompt());
                println!("{}", "-".repeat(60).dimmed());
            }

            Ok(())
        }

        TaskResolution::BrNotInstalled => {
            println!(
                "\n{} br command not found",
                "INFO".yellow().bold()
            );
            println!("\n{}", "The beads_rust (br) CLI is not installed.".dimmed());
            println!(
                "{}",
                "To install it, clone https://github.com/USERNAME/beads_rust".dimmed()
            );
            println!("{}", "and run `cargo install --path .`".dimmed());

            // Show with mock task for demonstration
            println!("\n{}", "Demonstrating with mock task:".blue().bold());
            let mock_task = create_mock_task(args.task_id.as_deref());
            print_task(&mock_task, &TaskSource::Mock);

            Ok(())
        }

        TaskResolution::NoTasksAvailable => {
            println!(
                "\n{} No tasks available",
                "INFO".yellow().bold()
            );
            println!(
                "\n{}",
                "The ready queue is empty. All tasks are either:".dimmed()
            );
            println!("{}", "  - Completed".dimmed());
            println!("{}", "  - Blocked by dependencies".dimmed());
            println!("{}", "  - Deferred".dimmed());
            println!(
                "\n{}",
                "Create a new task with `br create \"Task title\"` to test."
                    .white()
            );

            Err(anyhow!("No tasks available"))
        }

        TaskResolution::TaskNotFound(id) => {
            println!(
                "\n{} Task not found: {}",
                "ERROR".red().bold(),
                id.cyan()
            );
            println!(
                "\n{}",
                "The specified task ID was not found in the beads repository.".dimmed()
            );
            println!("{}", "Check the task ID and try again.".dimmed());
            println!("{}", "Use `br list` to see available tasks.".white());

            Err(anyhow!("Task not found: {}", id))
        }

        TaskResolution::Error(msg) => {
            println!(
                "\n{} {}",
                "ERROR".red().bold(),
                msg
            );

            Err(anyhow!("Task resolution failed: {}", msg))
        }
    }?;

    println!();
    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                    TASK RESOLUTION COMPLETE                          |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
    println!();

    // Show usage examples
    println!("{}", "Example Invocations:".yellow().bold());
    println!();
    println!(
        "  {} Get next ready task from br",
        "cargo run --example 30-task-resolution".dimmed()
    );
    println!(
        "  {} Get specific task",
        "cargo run --example 30-task-resolution -- bd-123".dimmed()
    );
    println!(
        "  {} With verbose output",
        "cargo run --example 30-task-resolution -- bd-123 -v".dimmed()
    );
    println!(
        "  {} Skip merge step",
        "cargo run --example 30-task-resolution -- --no-merge".dimmed()
    );
    println!();

    Ok(())
}
