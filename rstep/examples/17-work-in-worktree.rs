//! Example 17: Work in Worktree
//!
//! Demonstrates running Claude in a worktree instead of the main repo.
//!
//! This is the key insight: Claude operates in complete isolation. The main
//! branch is untouched until we explicitly merge.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Initializes a git repo with the bug-adder template
//! 3. Makes an initial commit on main
//! 4. Creates and locks a worktree for the task
//! 5. Sets Claude's working_directory to the WORKTREE path
//! 6. Claude fixes the bug (like ex-04)
//! 7. Verifies the fix is in the worktree
//! 8. Verifies main branch is still unchanged (still has the bug!)
//! 9. Unlocks and cleans up
//!
//! Run with:
//!   cargo run --example 17-work-in-worktree
//!
//! Key additions from previous examples:
//!   - Claude's cwd is set to the worktree, not the main repo
//!   - Main branch remains protected while Claude works
//!   - Side-by-side comparison proves isolation

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

/// Unlock a worktree
fn unlock_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    git(
        repo_path,
        &["worktree", "unlock", &worktree_path.to_string_lossy()],
    )?;
    Ok(())
}

/// Remove a worktree (force to remove uncommitted changes)
fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    git(
        repo_path,
        &["worktree", "remove", "--force", &worktree_path.to_string_lossy()],
    )?;
    Ok(())
}

/// Delete a branch
fn delete_branch(repo_path: &Path, branch_name: &str) -> Result<()> {
    git(repo_path, &["branch", "-d", branch_name])?;
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
        "|                 WORK IN WORKTREE EXAMPLE                 |"
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
        "|  Claude operates in worktree - main stays untouched!     |"
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

    // Show the original buggy code
    let main_lib_path = repo_path.join("src/lib.rs");
    let original_code = fs::read_to_string(&main_lib_path)?;
    println!("\n{}", "Original code in main (with bug):".red().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!("{}", original_code.trim());
    println!("{}", "-".repeat(60).dimmed());

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
    println!(
        "  {} {}",
        "Claude's cwd:".dimmed(),
        worktree_path.display().to_string().cyan()
    );
    println!(
        "  {} {}",
        "NOT:".dimmed(),
        repo_path.display().to_string().red()
    );

    // Configure Claude - KEY: cwd is the WORKTREE path, not the repo path!
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec![
            "Read".to_string(),
            "Write".to_string(),
            "Edit".to_string(),
        ])
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(&worktree_path) // <-- WORKTREE PATH, not repo_path!
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
    // 6. VERIFY WORKTREE HAS THE FIX
    // ============================================================

    println!(
        "\n{}",
        "Verifying isolation - this is the key!".yellow().bold()
    );
    println!("{}", "=".repeat(60).cyan());

    let worktree_lib_path = worktree_path.join("src/lib.rs");
    let worktree_code = fs::read_to_string(&worktree_lib_path)?;

    println!("\n{}", "WORKTREE src/lib.rs (should have fix):".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!("{}", worktree_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    let worktree_fixed = worktree_code.contains("a + b") && !worktree_code.contains("a - b");
    println!(
        "  {} {}",
        "Contains 'a + b':".dimmed(),
        if worktree_code.contains("a + b") {
            "YES".green().bold()
        } else {
            "NO".red().bold()
        }
    );
    println!(
        "  {} {}",
        "Bug fixed:".dimmed(),
        if worktree_fixed {
            "YES".green().bold()
        } else {
            "NO".red().bold()
        }
    );

    // ============================================================
    // 7. VERIFY MAIN STILL HAS THE BUG
    // ============================================================

    let main_code_after = fs::read_to_string(&main_lib_path)?;

    println!(
        "\n{}",
        "MAIN src/lib.rs (should still have bug):".red().bold()
    );
    println!("{}", "-".repeat(60).dimmed());
    println!("{}", main_code_after.trim());
    println!("{}", "-".repeat(60).dimmed());

    let main_still_buggy = main_code_after.contains("a - b");
    println!(
        "  {} {}",
        "Contains 'a - b' (bug):".dimmed(),
        if main_still_buggy {
            "YES".green().bold()
        } else {
            "NO".red().bold()
        }
    );
    println!(
        "  {} {}",
        "Main untouched:".dimmed(),
        if main_still_buggy {
            "YES".green().bold()
        } else {
            "NO".red().bold()
        }
    );

    println!("{}", "=".repeat(60).cyan());

    // ============================================================
    // 8. SUMMARY OF ISOLATION
    // ============================================================

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .blue()
            .bold()
    );
    println!(
        "{}",
        "|                    ISOLATION VERIFIED                    |"
            .blue()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .blue()
            .bold()
    );

    if worktree_fixed && main_still_buggy {
        println!(
            "\n{} {}",
            "SUCCESS:".green().bold(),
            "Worktree has fix, main is untouched!".white()
        );
        println!(
            "  {} {}",
            "Worktree:".dimmed(),
            "a + b (fixed)".green()
        );
        println!(
            "  {} {}",
            "Main:    ".dimmed(),
            "a - b (original bug)".red()
        );
    } else if !worktree_fixed {
        println!(
            "\n{} {}",
            "INCOMPLETE:".yellow().bold(),
            "Claude did not fix the bug in worktree".white()
        );
    } else {
        println!(
            "\n{} {}",
            "UNEXPECTED:".red().bold(),
            "Main was modified (should not happen)".white()
        );
    }

    // ============================================================
    // 9. TOKEN SUMMARY
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
    // 10. CLEANUP
    // ============================================================

    println!("\n{}", "Cleaning up...".yellow().bold());

    // Unlock worktree
    unlock_worktree(repo_path, &worktree_path)?;
    println!(
        "  {} {}",
        "Worktree unlocked:".dimmed(),
        "YES".green()
    );

    // Remove worktree
    remove_worktree(repo_path, &worktree_path)?;
    println!(
        "  {} {}",
        "Worktree removed:".dimmed(),
        "YES".green()
    );

    // Delete the branch
    delete_branch(repo_path, &branch_name)?;
    println!(
        "  {} {}",
        "Branch deleted:".dimmed(),
        "YES".green()
    );

    // ============================================================
    // 11. KEY TAKEAWAYS
    // ============================================================

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|               WORK IN WORKTREE COMPLETE                  |"
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
        "Claude's cwd is set to the WORKTREE, not the main repo"
    );
    println!(
        "  {} {}",
        "2.".cyan(),
        "All file operations happen inside the worktree"
    );
    println!(
        "  {} {}",
        "3.".cyan(),
        "Main branch remains completely untouched"
    );
    println!(
        "  {} {}",
        "4.".cyan(),
        "We can verify, then decide to merge (or discard)"
    );
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "This enables:".yellow().bold());
    println!(
        "  {} {}",
        "-".cyan(),
        "Safe experimentation (main is protected)"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Verification before merge (run tests first)"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Easy rollback (just delete the worktree)"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Parallel execution (multiple worktrees)"
    );

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Next: Merge the fix from worktree to main!".yellow().dimmed()
    );

    // Temp directory auto-deleted on drop
    Ok(())
}
