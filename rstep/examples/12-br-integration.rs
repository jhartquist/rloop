//! Example 12: br Integration
//!
//! Demonstrates how to get tasks from beads_rust (br) CLI.
//!
//! This example shows:
//! 1. Running `br ready --json --limit 1` to get the next ready task
//! 2. Parsing the JSON output into a Task struct
//! 3. Graceful handling when br is not installed
//! 4. Graceful handling when no tasks are available
//!
//! This is the foundation for later examples that will actually execute tasks.
//!
//! Run with:
//!   cargo run --example 12-br-integration

use anyhow::{anyhow, Result};
use colored::Colorize;
use serde::Deserialize;
use std::process::Command;

// ============================================================
// TYPES
// ============================================================

/// A task from beads_rust (br).
///
/// This is a simplified version of br's ReadyIssue that contains
/// the fields we care about for task execution.
#[derive(Debug, Deserialize)]
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
    ///
    /// Combines title, description, and acceptance criteria into a clear prompt.
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
// BR CLI INTEGRATION
// ============================================================

/// Result of attempting to get a task from br.
#[derive(Debug)]
pub enum BrResult {
    /// Successfully got a task
    Task(Task),
    /// br command not found (not installed)
    NotInstalled,
    /// No tasks available (empty ready queue)
    NoTasks,
    /// br command failed with an error
    Error(String),
}

/// Get the next ready task from br.
///
/// Runs `br ready --json --limit 1` and parses the result.
pub fn get_ready_task() -> BrResult {
    // Try to run br
    let output = match Command::new("br")
        .args(["ready", "--json", "--limit", "1"])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            // Check if it's a "not found" error
            if e.kind() == std::io::ErrorKind::NotFound {
                return BrResult::NotInstalled;
            }
            return BrResult::Error(format!("Failed to run br: {}", e));
        }
    };

    // Check exit status
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Check for common "no beads repo" error
        if stderr.contains("not a beads repository")
            || stderr.contains("no .beads directory")
            || stderr.contains("could not find")
        {
            return BrResult::Error("Not in a beads repository".to_string());
        }
        return BrResult::Error(format!("br failed: {}", stderr.trim()));
    }

    // Parse JSON output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();

    // Empty output or empty array means no tasks
    if stdout.is_empty() || stdout == "[]" {
        return BrResult::NoTasks;
    }

    // br outputs an array, even with --limit 1
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

fn print_task(task: &Task) {
    println!("\n{}", "Task Details:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
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
    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                  BR INTEGRATION EXAMPLE                   |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                           |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  Get the next ready task from beads_rust (br) CLI.        |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

    println!("\n{}", "Querying br for ready tasks...".yellow());

    match get_ready_task() {
        BrResult::Task(task) => {
            println!(
                "\n{} Found a task!",
                "SUCCESS".green().bold()
            );
            print_task(&task);

            // Show what the prompt would look like
            println!("\n{}", "Generated Prompt:".blue().bold());
            println!("{}", "-".repeat(60).dimmed());
            println!("{}", task.to_prompt());
            println!("{}", "-".repeat(60).dimmed());

            println!(
                "\n{}",
                "In a real step, this prompt would be sent to Claude."
                    .dimmed()
            );

            Ok(())
        }
        BrResult::NotInstalled => {
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
            println!(
                "\n{}",
                "For now, this example demonstrates the integration pattern."
                    .white()
            );

            // Show what would happen with a mock task
            println!("\n{}", "Mock Task (for demonstration):".blue().bold());
            let mock_task = Task {
                id: "bd-abc123".to_string(),
                title: "Fix the bug in add function".to_string(),
                description: Some("The add function returns a-b instead of a+b".to_string()),
                status: "open".to_string(),
                priority: 2,
                issue_type: "bug".to_string(),
                acceptance_criteria: Some("- add(2, 3) should return 5\n- add(-1, 1) should return 0".to_string()),
            };
            print_task(&mock_task);

            Ok(())
        }
        BrResult::NoTasks => {
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

            Ok(())
        }
        BrResult::Error(msg) => {
            println!(
                "\n{} {}",
                "ERROR".red().bold(),
                msg
            );
            Err(anyhow!("br integration failed: {}", msg))
        }
    }
}
