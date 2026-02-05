//! Example 21: Full Worktree Lifecycle
//!
//! Combines all worktree operations into one coherent flow, demonstrating
//! the complete lifecycle from RSTEP.md:
//!
//! 1. CREATE:  git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}
//! 2. LOCK:    git worktree lock {path} --reason "rstep active"
//! 3. WORK:    Claude operates in worktree
//! 4. VERIFY:  cargo test in worktree
//! 5. MERGE:   git checkout main && git merge --squash && git commit
//! 6. CLEANUP: unlock, remove worktree, delete branch
//!
//! This example shows the entire lifecycle in a single executable flow,
//! making it clear how all the pieces fit together.
//!
//! Run with:
//!   cargo run --example 21-full-worktree-lifecycle
//!
//! This example uses the rstep::git module for all git operations.

use anyhow::{Context, Result};
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, ContentBlock, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use rstep::git::{
    branch_name, cleanup_worktree, commit_changes, create_worktree, current_branch, get_diff,
    get_log, init_repo, list_branches, list_worktrees, lock_worktree, merge_to_main,
    unlock_worktree, last_commit,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tracing::{debug, info};

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

/// Run verification (cargo test) in the given directory
fn run_verification(path: &Path) -> Result<bool> {
    let output = Command::new("cargo")
        .args(["test"])
        .current_dir(path)
        .output()
        .context("Failed to run cargo test")?;

    Ok(output.status.success())
}

/// Print a phase banner
fn print_phase(phase: &str, description: &str) {
    println!();
    println!("{}", "=".repeat(60).cyan());
    println!(
        "{} {} {}",
        "PHASE:".cyan().bold(),
        phase.yellow().bold(),
        format!("- {}", description).white()
    );
    println!("{}", "=".repeat(60).cyan());
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
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|              FULL WORKTREE LIFECYCLE EXAMPLE             |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                          |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  CREATE -> LOCK -> WORK -> VERIFY -> MERGE -> CLEANUP   |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

    // ============================================================
    // SETUP: Create temp directory and initialize repo
    // ============================================================

    let temp = tempdir()?;
    let repo_path = temp.path();
    info!("{}", "Setting up temp directory...".cyan().bold());
    println!(
        "\n{} {}",
        "Temp directory:".yellow().bold(),
        repo_path.display().to_string().white()
    );

    // Copy template
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    debug!("Template source: {:?}", template_dir);
    copy_dir_all(&template_dir, repo_path)?;
    info!(
        "{} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    // Initialize git repo (using library function)
    println!("\n{}", "Initializing git repository...".yellow().bold());
    init_repo(repo_path)?;

    let main_branch = current_branch(repo_path)?;
    let initial_commit = last_commit(repo_path)?;
    println!(
        "  {} {}",
        "Branch:".dimmed(),
        main_branch.green().bold()
    );
    println!(
        "  {} {}",
        "Initial commit:".dimmed(),
        initial_commit.white()
    );

    let task_id = "bd-fix-add";
    let task_branch = branch_name(task_id);

    // ============================================================
    // PHASE 1: CREATE
    // ============================================================

    print_phase("CREATE", "git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}");

    let worktree_path = create_worktree(repo_path, task_id)?;

    println!(
        "  {} {}",
        "Task ID:".dimmed(),
        task_id.cyan().bold()
    );
    println!(
        "  {} {}",
        "Worktree path:".dimmed(),
        worktree_path.display().to_string().white()
    );
    println!(
        "  {} {}",
        "Branch name:".dimmed(),
        task_branch.cyan()
    );

    // Show worktree list
    println!("\n{}", "Worktree list:".green().bold());
    let worktrees = list_worktrees(repo_path)?;
    for line in worktrees.lines() {
        println!("  {}", line);
    }

    // ============================================================
    // PHASE 2: LOCK
    // ============================================================

    print_phase("LOCK", "git worktree lock {path} --reason \"rstep active\"");

    lock_worktree(&worktree_path, "rstep active")?;

    println!(
        "  {} {}",
        "Locked with reason:".dimmed(),
        "rstep active".cyan().bold()
    );
    println!(
        "  {} {}",
        "Protection:".dimmed(),
        "Prevents accidental deletion during work".white()
    );

    // ============================================================
    // PHASE 3: WORK
    // ============================================================

    print_phase("WORK", "Claude operates in worktree (isolated from main)");

    println!(
        "  {} {}",
        "Claude's cwd:".dimmed(),
        worktree_path.display().to_string().cyan()
    );
    println!(
        "  {} {}",
        "Main repo:".dimmed(),
        repo_path.display().to_string().white()
    );
    println!(
        "  {} {}",
        "Isolation:".dimmed(),
        "Changes only affect worktree branch".green()
    );

    // Configure Claude - cwd is the WORKTREE path
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec![
            "Read".to_string(),
            "Write".to_string(),
            "Edit".to_string(),
        ])
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(&worktree_path)
        .build();

    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // Send our query
    let query =
        "This is a Rust project. Read src/lib.rs, find the bug in the add function, and fix it.";
    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        query
    );
    client.query(query).await?;

    // Receive response
    println!("\n{}", "Claude's response:".green().bold());
    println!("{}", "-".repeat(60).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
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
                            if let Some(input) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                total_input_tokens += input;
                            }
                            if let Some(output) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                total_output_tokens += output;
                            }
                        }
                    }
                    Message::Result(result) => {
                        if let Some(usage) = &result.usage {
                            if let Some(input) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                total_input_tokens += input;
                            }
                            if let Some(output) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                total_output_tokens += output;
                            }
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                }
            }
        }
    }

    // Show tool uses
    if !tool_uses.is_empty() {
        println!("{}", "Tools used:".magenta().bold());
        for tool in &tool_uses {
            println!("  {} {}", "->".magenta(), tool);
        }
        println!();
    }

    // Print response (truncated)
    if !response_text.is_empty() {
        let truncated = if response_text.len() > 300 {
            format!("{}...", &response_text[..300])
        } else {
            response_text.clone()
        };
        println!("{}", truncated);
    }
    println!("{}", "-".repeat(60).dimmed());

    client.disconnect().await?;

    // Check if fix was applied
    let worktree_lib_path = worktree_path.join("src/lib.rs");
    let worktree_code = fs::read_to_string(&worktree_lib_path)?;
    let bug_fixed = worktree_code.contains("a + b") && !worktree_code.contains("a - b");

    if !bug_fixed {
        println!(
            "\n{} {}",
            "WARNING:".yellow().bold(),
            "Claude did not fix the bug - aborting".white()
        );
        let _ = cleanup_worktree(repo_path, task_id);
        return Ok(());
    }

    println!(
        "\n{} {}",
        "Bug fixed:".green(),
        "YES - worktree has correct code".green().bold()
    );

    // Commit changes in worktree (using library function)
    println!("\n{}", "Committing changes in worktree...".yellow().bold());
    let commit_message = "fix: correct add function to use + instead of -";
    let commit_hash = commit_changes(&worktree_path, commit_message)?;
    println!(
        "  {} {}",
        "Commit:".dimmed(),
        commit_hash[..12].cyan().bold()
    );

    // ============================================================
    // PHASE 4: VERIFY
    // ============================================================

    print_phase("VERIFY", "cargo test in worktree");

    println!("{}", "Running cargo test...".yellow());

    let tests_pass = run_verification(&worktree_path)?;

    if tests_pass {
        println!(
            "  {} {}",
            "Tests:".dimmed(),
            "PASSED".green().bold()
        );
    } else {
        println!(
            "  {} {}",
            "Tests:".dimmed(),
            "FAILED".red().bold()
        );
        println!(
            "\n{} {}",
            "ABORT:".red().bold(),
            "Tests failed - not merging to main".white()
        );
        let _ = cleanup_worktree(repo_path, task_id);
        return Ok(());
    }

    // ============================================================
    // PHASE 5: MERGE
    // ============================================================

    print_phase("MERGE", "git checkout main && git merge --squash && git commit");

    // Unlock worktree first (required before checkout main)
    unlock_worktree(&worktree_path)?;
    println!(
        "  {} {}",
        "Worktree unlocked:".dimmed(),
        "YES (required for checkout)".white()
    );

    let task_summary = "fix add function";
    merge_to_main(repo_path, task_id, task_summary)?;

    println!(
        "  {} {}",
        "Squash merge:".dimmed(),
        "SUCCESS".green().bold()
    );
    println!(
        "  {} {}",
        "Commit message:".dimmed(),
        format!("task: {}", task_summary).cyan()
    );

    // Show git log on main (using library function)
    println!("\n{}", "Git log (main branch):".green().bold());
    let log = get_log(repo_path, 3)?;
    for line in log.lines() {
        println!("  {}", line.white());
    }

    // ============================================================
    // PHASE 6: CLEANUP
    // ============================================================

    print_phase("CLEANUP", "unlock, remove worktree, delete branch");

    // Show state before cleanup (using library functions)
    println!("\n{}", "Before cleanup:".yellow().bold());
    let worktrees_before = list_worktrees(repo_path)?;
    println!("{}", "  Worktrees:");
    for line in worktrees_before.lines() {
        println!("    {}", line);
    }
    let branches_before = list_branches(repo_path)?;
    println!("{}", "  Branches:");
    for line in branches_before.lines() {
        println!("    {}", line.trim());
    }

    // Perform cleanup (using library function)
    let (worktree_removed, branch_deleted) = cleanup_worktree(repo_path, task_id)?;

    println!("\n{}", "Cleanup operations:".yellow().bold());
    println!(
        "  {} {} {}",
        "1.".cyan(),
        "git worktree unlock:".dimmed(),
        "(already done before merge)".white()
    );
    println!(
        "  {} {} {}",
        "2.".cyan(),
        "git worktree remove:".dimmed(),
        if worktree_removed {
            "SUCCESS".green().bold()
        } else {
            "FAILED".red().bold()
        }
    );
    println!(
        "  {} {} {}",
        "3.".cyan(),
        "git branch -D:".dimmed(),
        if branch_deleted {
            "SUCCESS".green().bold()
        } else {
            "FAILED".red().bold()
        }
    );

    // Show state after cleanup
    println!("\n{}", "After cleanup:".green().bold());
    let worktrees_after = list_worktrees(repo_path)?;
    println!("{}", "  Worktrees:");
    for line in worktrees_after.lines() {
        println!("    {}", line);
    }
    let branches_after = list_branches(repo_path)?;
    println!("{}", "  Branches:");
    for line in branches_after.lines() {
        println!("    {}", line.trim());
    }

    // Verify directory is gone
    let worktree_dir_exists = worktree_path.exists();
    println!(
        "\n{} {}",
        "Worktree directory gone:".dimmed(),
        if !worktree_dir_exists {
            "YES".green().bold()
        } else {
            "NO (unexpected!)".red().bold()
        }
    );

    // ============================================================
    // FINAL VERIFICATION
    // ============================================================

    println!();
    println!("{}", "=".repeat(60).green());
    println!(
        "{}",
        "FINAL VERIFICATION".green().bold()
    );
    println!("{}", "=".repeat(60).green());

    // Verify main has the fix
    let main_lib_path = repo_path.join("src/lib.rs");
    let main_code = fs::read_to_string(&main_lib_path)?;
    let main_has_fix = main_code.contains("a + b") && !main_code.contains("a - b");

    println!(
        "\n{} {}",
        "Main has fix:".dimmed(),
        if main_has_fix {
            "YES".green().bold()
        } else {
            "NO".red().bold()
        }
    );

    // Show diff (using library function)
    println!("\n{}", "Changes merged to main:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let diff = get_diff(repo_path, &initial_commit, "HEAD")?;
    for line in diff.lines() {
        let colored_line = if line.starts_with('+') && !line.starts_with("+++") {
            line.green().to_string()
        } else if line.starts_with('-') && !line.starts_with("---") {
            line.red().to_string()
        } else if line.starts_with("@@") {
            line.cyan().to_string()
        } else {
            line.white().to_string()
        };
        println!("{}", colored_line);
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // SUMMARY
    // ============================================================

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|              FULL WORKTREE LIFECYCLE COMPLETE            |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );

    println!("\n{}", "Complete lifecycle:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {} {}",
        "1. CREATE".cyan().bold(),
        "git worktree add".white(),
        "- Isolated branch for task".dimmed()
    );
    println!(
        "  {} {} {}",
        "2. LOCK  ".cyan().bold(),
        "git worktree lock".white(),
        "- Prevent accidental deletion".dimmed()
    );
    println!(
        "  {} {} {}",
        "3. WORK  ".cyan().bold(),
        "Claude in worktree".white(),
        "- Main stays untouched".dimmed()
    );
    println!(
        "  {} {} {}",
        "4. VERIFY".cyan().bold(),
        "cargo test".white(),
        "- Ensure quality before merge".dimmed()
    );
    println!(
        "  {} {} {}",
        "5. MERGE ".cyan().bold(),
        "git merge --squash".white(),
        "- One clean commit to main".dimmed()
    );
    println!(
        "  {} {} {}",
        "6. CLEANUP".cyan().bold(),
        "remove + delete branch".white(),
        "- Clean state for next task".dimmed()
    );
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "Tokens used:".blue().bold());
    println!(
        "  {} {}",
        "Input:".dimmed(),
        total_input_tokens.to_string().white()
    );
    println!(
        "  {} {}",
        "Output:".dimmed(),
        total_output_tokens.to_string().white()
    );
    println!(
        "  {} {}",
        "Total:".dimmed(),
        (total_input_tokens + total_output_tokens)
            .to_string()
            .cyan()
            .bold()
    );

    info!("{}", "Done.".green());

    // Temp directory auto-deleted on drop
    Ok(())
}
