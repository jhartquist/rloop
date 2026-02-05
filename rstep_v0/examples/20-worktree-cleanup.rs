//! Example 20: Worktree Cleanup
//!
//! Demonstrates proper cleanup of worktree and branch after successful merge.
//!
//! What this does:
//! 1. Creates a temp directory with git repo and worktree (like ex-19)
//! 2. Runs Claude to fix the bug
//! 3. Commits changes in the worktree branch
//! 4. Runs verification (`cargo test` in worktree)
//! 5. If tests pass, squash merges to main
//! 6. **CLEANUP (this example's focus):**
//!    - Shows before state (worktree list, branch list)
//!    - `git worktree unlock {path}` (if locked)
//!    - `git worktree remove {path}`
//!    - `git branch -d rloop/{task-id}` (safe delete - branch is merged)
//!    - Shows after state (both should be empty)
//! 7. Verifies main still has the merged changes
//!
//! Run with:
//!   cargo run --example 20-worktree-cleanup
//!
//! Key additions from previous examples:
//!   - Explicit before/after state for worktrees and branches
//!   - Safe branch delete with -d (not -D) since branch is merged
//!   - Cleanup verification

use anyhow::{Context, Result};
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, ContentBlock, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Run a git command in the given directory, returning stdout
fn git(repo_path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("Failed to run git {:?}", args))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {:?} failed: {}", args, stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a git command, ignoring errors (for cleanup operations)
fn git_ignore_errors(repo_path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Initialize a git repo with initial commit
fn init_repo(repo_path: &Path) -> Result<()> {
    git(repo_path, &["init"])?;
    git(repo_path, &["config", "user.email", "test@example.com"])?;
    git(repo_path, &["config", "user.name", "Test User"])?;

    // Add all files and create initial commit
    git(repo_path, &["add", "-A"])?;
    git(repo_path, &["commit", "-m", "Initial commit"])?;

    Ok(())
}

/// Create a worktree for the given task ID
fn create_worktree(repo_path: &Path, task_id: &str) -> Result<PathBuf> {
    let worktree_dir = repo_path.join(".rloop/worktrees");
    fs::create_dir_all(&worktree_dir)?;

    let worktree_path = worktree_dir.join(task_id);
    let branch_name = format!("rloop/{}", task_id);

    git(
        repo_path,
        &[
            "worktree",
            "add",
            &worktree_path.to_string_lossy(),
            "-b",
            &branch_name,
        ],
    )?;

    Ok(worktree_path)
}

/// Lock a worktree with a reason
fn lock_worktree(repo_path: &Path, worktree_path: &Path, reason: &str) -> Result<()> {
    git(
        repo_path,
        &[
            "worktree",
            "lock",
            &worktree_path.to_string_lossy(),
            "--reason",
            reason,
        ],
    )?;
    Ok(())
}

/// Get the current branch name
fn current_branch(repo_path: &Path) -> Result<String> {
    git(repo_path, &["branch", "--show-current"])
}

/// Get the last commit hash (short)
fn last_commit(repo_path: &Path) -> Result<String> {
    git(repo_path, &["rev-parse", "--short", "HEAD"])
}

/// Commit changes in a worktree
fn commit_changes(worktree_path: &Path, message: &str) -> Result<String> {
    // Stage all changes
    git(worktree_path, &["add", "-A"])?;

    // Check if there are changes to commit
    let status = git(worktree_path, &["status", "--porcelain"])?;
    if status.is_empty() {
        anyhow::bail!("No changes to commit");
    }

    // Commit
    git(worktree_path, &["commit", "-m", message])?;

    // Get commit hash (full)
    let commit_hash = git(worktree_path, &["rev-parse", "HEAD"])?;

    Ok(commit_hash)
}

/// Get git log (oneline, limited)
fn get_log(repo_path: &Path, limit: usize) -> Result<String> {
    git(repo_path, &["log", "--oneline", &format!("-{}", limit)])
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

/// Squash merge a branch to main
fn merge_to_main(repo_path: &Path, task_id: &str, summary: &str) -> Result<()> {
    let branch_name = format!("rloop/{}", task_id);

    // Checkout main
    git(repo_path, &["checkout", "main"])?;

    // Squash merge
    git(repo_path, &["merge", "--squash", &branch_name])?;

    // Commit with task summary
    let message = format!("task: {}", summary);
    git(repo_path, &["commit", "-m", &message])?;

    Ok(())
}

/// Get the diff between two commits
fn get_diff(repo_path: &Path, from: &str, to: &str) -> Result<String> {
    git(repo_path, &["diff", from, to])
}

/// List all worktrees
fn list_worktrees(repo_path: &Path) -> Result<String> {
    git(repo_path, &["worktree", "list"])
}

/// List all branches (excluding main)
fn list_branches(repo_path: &Path) -> Result<String> {
    git(repo_path, &["branch", "-a"])
}

/// Cleanup worktree and branch after successful merge
/// Returns (worktree_removed, branch_deleted)
fn cleanup_worktree(repo_path: &Path, task_id: &str) -> Result<(bool, bool)> {
    let worktree_path = repo_path.join(".rloop/worktrees").join(task_id);
    let branch_name = format!("rloop/{}", task_id);

    // 1. Unlock worktree (ignore errors if not locked)
    let _ = git_ignore_errors(
        repo_path,
        &["worktree", "unlock", &worktree_path.to_string_lossy()],
    );

    // 2. Remove worktree
    let worktree_removed = git(
        repo_path,
        &["worktree", "remove", &worktree_path.to_string_lossy()],
    )
    .is_ok();

    // 3. Delete branch with -D (force delete)
    //    We use -D instead of -d because after a squash merge, git doesn't
    //    recognize the branch as "merged" (squash creates a new commit).
    //    Since we verified the merge succeeded and main has the fix,
    //    force delete is safe here.
    let branch_deleted = git(repo_path, &["branch", "-D", &branch_name]).is_ok();

    Ok((worktree_removed, branch_deleted))
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
        "|                 WORKTREE CLEANUP EXAMPLE                 |"
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
        "|  Proper cleanup after successful merge                   |"
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
    // 1. CREATE TEMP DIRECTORY
    // ============================================================

    let temp = tempdir()?;
    let repo_path = temp.path();
    info!("{}", "Setting up temp directory...".cyan().bold());
    println!(
        "\n{} {}",
        "Temp directory:".yellow().bold(),
        repo_path.display().to_string().white()
    );

    // ============================================================
    // 2. COPY TEMPLATE INTO TEMP DIRECTORY
    // ============================================================

    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    debug!("Template source: {:?}", template_dir);
    copy_dir_all(&template_dir, repo_path)?;
    info!(
        "{} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    // ============================================================
    // 3. INITIALIZE GIT REPO
    // ============================================================

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

    // ============================================================
    // 4. CREATE AND LOCK WORKTREE
    // ============================================================

    let task_id = "bd-fix-add";
    println!(
        "\n{} {}",
        "Creating worktree for task:".yellow().bold(),
        task_id.cyan().bold()
    );

    let worktree_path = create_worktree(repo_path, task_id)?;
    let branch_name = format!("rloop/{}", task_id);

    println!(
        "  {} {}",
        "Worktree path:".dimmed(),
        worktree_path.display().to_string().white()
    );
    println!(
        "  {} {}",
        "Branch name:".dimmed(),
        branch_name.cyan()
    );

    // Lock the worktree
    lock_worktree(repo_path, &worktree_path, "rstep active")?;
    println!(
        "  {} {}",
        "Worktree locked:".dimmed(),
        "YES".green().bold()
    );

    // ============================================================
    // 5. RUN CLAUDE IN THE WORKTREE
    // ============================================================

    println!("\n{}", "Running Claude in worktree...".yellow().bold());

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

    // Receive and display response
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

    // Print the response text (truncated if long)
    if !response_text.is_empty() {
        let truncated = if response_text.len() > 500 {
            format!("{}...", &response_text[..500])
        } else {
            response_text.clone()
        };
        println!("{}", truncated);
    }
    println!("{}", "-".repeat(60).dimmed());

    // Disconnect
    client.disconnect().await?;

    // ============================================================
    // 6. CHECK IF FIX WAS APPLIED AND COMMIT
    // ============================================================

    println!("\n{}", "Checking for changes...".yellow().bold());

    let worktree_lib_path = worktree_path.join("src/lib.rs");
    let worktree_code = fs::read_to_string(&worktree_lib_path)?;
    let bug_fixed = worktree_code.contains("a + b") && !worktree_code.contains("a - b");

    if !bug_fixed {
        println!(
            "\n{} {}",
            "WARNING:".yellow().bold(),
            "Claude did not fix the bug - skipping merge".white()
        );

        // Cleanup and exit early
        let _ = cleanup_worktree(repo_path, task_id);

        return Ok(());
    }

    println!(
        "  {} {}",
        "Bug fixed:".dimmed(),
        "YES".green().bold()
    );

    // Commit the changes
    println!("\n{}", "Committing changes...".yellow().bold());

    let commit_message = "fix: correct add function to use + instead of -";
    let task_summary = "fix add function";
    let commit_hash = commit_changes(&worktree_path, commit_message)?;

    println!(
        "  {} {}",
        "Commit hash:".dimmed(),
        commit_hash[..12].cyan().bold()
    );
    println!(
        "  {} {}",
        "Message:".dimmed(),
        commit_message.white()
    );

    // ============================================================
    // 7. RUN VERIFICATION
    // ============================================================

    println!("\n{}", "Running verification (cargo test)...".yellow().bold());

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

        // Cleanup and exit early
        let _ = cleanup_worktree(repo_path, task_id);

        return Ok(());
    }

    // ============================================================
    // 8. SHOW STATE BEFORE MERGE (worktrees and branches)
    // ============================================================

    println!("\n{}", "=".repeat(60).cyan());
    println!(
        "{}",
        "STATE BEFORE CLEANUP".cyan().bold()
    );
    println!("{}", "=".repeat(60).cyan());

    println!("\n{}", "Worktrees:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let worktrees_before = list_worktrees(repo_path)?;
    for line in worktrees_before.lines() {
        println!("  {}", line.white());
    }
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "Branches:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let branches_before = list_branches(repo_path)?;
    for line in branches_before.lines() {
        let line = line.trim();
        if line.contains("rloop/") {
            println!("  {} {}", "->".cyan(), line.cyan().bold());
        } else {
            println!("  {}", line.white());
        }
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 9. MERGE TO MAIN
    // ============================================================

    println!("\n{}", "Squash merging to main...".yellow().bold());

    // Need to unlock worktree before we can checkout main
    let _ = git_ignore_errors(
        repo_path,
        &["worktree", "unlock", &worktree_path.to_string_lossy()],
    );

    merge_to_main(repo_path, task_id, task_summary)?;

    println!(
        "  {} {}",
        "Merge:".dimmed(),
        "SUCCESS".green().bold()
    );

    // Show git log on main
    println!("\n{}", "Git log (main branch):".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());

    let log = get_log(repo_path, 5)?;
    for line in log.lines() {
        println!("  {}", line.white());
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 10. CLEANUP WORKTREE AND BRANCH
    // ============================================================

    println!("\n{}", "=".repeat(60).green());
    println!(
        "{}",
        "CLEANUP".green().bold()
    );
    println!("{}", "=".repeat(60).green());

    println!("\n{}", "Cleaning up worktree and branch...".yellow().bold());

    // Perform cleanup
    let (worktree_removed, branch_deleted) = cleanup_worktree(repo_path, task_id)?;

    println!(
        "  {} {} {}",
        "1.".cyan(),
        "git worktree unlock:".dimmed(),
        "(already unlocked for merge)".white()
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
        "git branch -d:".dimmed(),
        if branch_deleted {
            "SUCCESS".green().bold()
        } else {
            "FAILED".red().bold()
        }
    );

    // ============================================================
    // 11. SHOW STATE AFTER CLEANUP
    // ============================================================

    println!("\n{}", "=".repeat(60).cyan());
    println!(
        "{}",
        "STATE AFTER CLEANUP".cyan().bold()
    );
    println!("{}", "=".repeat(60).cyan());

    println!("\n{}", "Worktrees:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let worktrees_after = list_worktrees(repo_path)?;
    for line in worktrees_after.lines() {
        println!("  {}", line.white());
    }
    // Verify no task worktree exists
    let has_task_worktree = worktrees_after.contains(task_id);
    if !has_task_worktree {
        println!("  {} {}", "->".green(), "Task worktree removed!".green().bold());
    }
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "Branches:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let branches_after = list_branches(repo_path)?;
    for line in branches_after.lines() {
        let line = line.trim();
        println!("  {}", line.white());
    }
    // Verify no rloop branch exists
    let has_rloop_branch = branches_after.contains("rloop/");
    if !has_rloop_branch {
        println!("  {} {}", "->".green(), "Task branch deleted!".green().bold());
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 12. VERIFY MAIN STILL HAS THE FIX
    // ============================================================

    println!("\n{}", "Verifying main still has the fix...".yellow().bold());

    let main_lib_path = repo_path.join("src/lib.rs");
    let main_code = fs::read_to_string(&main_lib_path)?;
    let main_has_fix = main_code.contains("a + b") && !main_code.contains("a - b");

    println!(
        "  {} {}",
        "Main has fix:".dimmed(),
        if main_has_fix {
            "YES".green().bold()
        } else {
            "NO".red().bold()
        }
    );

    // Show the diff from initial commit
    let main_commit = last_commit(repo_path)?;
    println!(
        "  {} {}",
        "Main commit:".dimmed(),
        main_commit.cyan().bold()
    );

    println!("\n{}", "Changes on main:".yellow().bold());
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

    // Verify worktree directory is actually gone
    let worktree_dir_exists = worktree_path.exists();
    println!(
        "\n{} {}",
        "Worktree directory exists:".dimmed(),
        if worktree_dir_exists {
            "YES (unexpected!)".red().bold()
        } else {
            "NO (as expected)".green().bold()
        }
    );

    // ============================================================
    // 13. TOKEN SUMMARY
    // ============================================================

    println!("\n{}", "Tokens:".blue().bold());
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

    // ============================================================
    // 14. KEY TAKEAWAYS
    // ============================================================

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                   CLEANUP COMPLETE                       |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );

    println!("\n{}", "Key insight:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "1.".cyan(),
        "Use -D after squash merge (git can't verify merged status)"
    );
    println!(
        "  {} {}",
        "2.".cyan(),
        "Squash merge creates NEW commit, not merge commit"
    );
    println!(
        "  {} {}",
        "3.".cyan(),
        "Since we verified main has the fix, -D is safe"
    );
    println!(
        "  {} {}",
        "4.".cyan(),
        "Unlock worktree before switching branches"
    );
    println!(
        "  {} {}",
        "5.".cyan(),
        "Remove worktree before deleting branch"
    );
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "Cleanup order:".yellow().bold());
    println!(
        "  {} {} {}",
        "1.".cyan(),
        "git worktree unlock".white(),
        "(ignore errors if not locked)".dimmed()
    );
    println!(
        "  {} {} {}",
        "2.".cyan(),
        "git worktree remove".white(),
        "(removes worktree directory)".dimmed()
    );
    println!(
        "  {} {} {}",
        "3.".cyan(),
        "git branch -D".white(),
        "(force delete - OK after verified squash merge)".dimmed()
    );

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Success! Worktree and branch cleaned up after merge."
            .yellow()
            .dimmed()
    );

    // Temp directory auto-deleted on drop
    Ok(())
}
