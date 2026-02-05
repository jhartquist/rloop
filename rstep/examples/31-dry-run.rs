//! Example 31: Dry Run Implementation
//!
//! Demonstrates the --dry-run flag that shows the full execution plan
//! without making any changes.
//!
//! Dry run is useful for:
//! - Verifying task resolution before execution
//! - Previewing the config that will be used
//! - Seeing the exact prompt that will be sent to Claude
//! - Understanding the execution flow
//!
//! Run with:
//!   cargo run --example 31-dry-run -- --dry-run
//!   cargo run --example 31-dry-run -- bd-123 --dry-run
//!   cargo run --example 31-dry-run -- -n -v

use anyhow::{anyhow, Result};
use clap::Parser;
use colored::Colorize;
use rstep::step::{load_config, Config, Task};
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
// TASK RESOLUTION (from example 30)
// ============================================================

/// Result of attempting to resolve a task.
#[derive(Debug)]
pub enum TaskResolution {
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
pub fn resolve_task(args: &Args) -> TaskResolution {
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

    println!(
        "  {} {}",
        "Max turns:".white(),
        config.step.max_turns.to_string().dimmed()
    );

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
    println!("\n{}", "[PROMPT PREVIEW]".cyan().bold());
    println!("{}", "-".repeat(40).dimmed());

    let prompt = task.to_prompt();
    // Show first 500 chars or full prompt if shorter
    let preview = if prompt.len() > 500 {
        format!("{}...\n\n  (truncated, {} total chars)", &prompt[..500], prompt.len())
    } else {
        prompt.clone()
    };

    for line in preview.lines() {
        println!("  {}", line);
    }
    println!("{}", "-".repeat(40).dimmed());

    // Show full prompt in verbose mode
    if args.verbose && prompt.len() > 500 {
        println!("\n{}", "[FULL PROMPT]".cyan().bold());
        println!("{}", "-".repeat(40).dimmed());
        for line in prompt.lines() {
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
        "|                         DRY RUN MODE                                 |"
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
        "|  Preview execution plan without making changes                       |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );

    // Load config
    let config_path = args.config.clone().unwrap_or_else(|| PathBuf::from("."));
    let config = load_config(&config_path);

    if args.verbose {
        println!("\n{}", "Config loaded from:".yellow().bold());
        println!(
            "  {}",
            config_path.join(".rloop/config.toml").display().to_string().dimmed()
        );
    }

    // Resolve task
    println!("\n{}", "Resolving task...".yellow());

    let resolution = resolve_task(&args);

    let (task, source) = match resolution {
        TaskResolution::Resolved { task, source } => {
            println!(
                "{} Task resolved: {}",
                "OK".green().bold(),
                task.id.cyan()
            );
            (task, source)
        }

        TaskResolution::BrNotInstalled => {
            println!(
                "\n{} br command not found - using mock task",
                "INFO".yellow().bold()
            );
            let task = create_mock_task(args.task_id.as_deref());
            (task, TaskSource::Mock)
        }

        TaskResolution::NoTasksAvailable => {
            println!(
                "\n{} No tasks available in ready queue",
                "ERROR".red().bold()
            );
            return Err(anyhow!("No tasks available"));
        }

        TaskResolution::TaskNotFound(id) => {
            println!(
                "\n{} Task not found: {}",
                "ERROR".red().bold(),
                id.cyan()
            );
            return Err(anyhow!("Task not found: {}", id));
        }

        TaskResolution::Error(msg) => {
            println!(
                "\n{} {}",
                "ERROR".red().bold(),
                msg
            );
            return Err(anyhow!("Task resolution failed: {}", msg));
        }
    };

    // Check if dry run mode
    if args.dry_run {
        print_dry_run(&task, &source, &config, &args);
    } else {
        // Not dry run - show what would execute
        println!("\n{}", "Would execute step for task:".yellow().bold());
        println!("  {} {}", "ID:".dimmed(), task.id.cyan().bold());
        println!("  {} {}", "Title:".dimmed(), task.title);
        println!();
        println!(
            "{}",
            "Execution not implemented in this example.".dimmed()
        );
        println!(
            "{}",
            "Use --dry-run (-n) to see the full execution plan.".white()
        );
        println!();
    }

    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                         EXAMPLE COMPLETE                             |"
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
        "  {} Preview with mock task",
        "cargo run --example 31-dry-run -- --dry-run".dimmed()
    );
    println!(
        "  {} Preview specific task",
        "cargo run --example 31-dry-run -- bd-123 -n".dimmed()
    );
    println!(
        "  {} With verbose output",
        "cargo run --example 31-dry-run -- -n -v".dimmed()
    );
    println!(
        "  {} Preview without merge",
        "cargo run --example 31-dry-run -- -n --no-merge".dimmed()
    );
    println!();

    Ok(())
}
