//! Example 16: Worktree Lock
//!
//! Demonstrates locking/unlocking worktrees during work.
//!
//! Why lock? Prevents accidental deletion or `git worktree prune` from
//! removing a worktree while work is in progress.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Initializes a git repo with the bug-adder template
//! 3. Creates a worktree for a task
//! 4. Locks the worktree with a reason: `git worktree lock {path} --reason "rstep active"`
//! 5. Attempts to remove the locked worktree (demonstrates protection)
//! 6. Unlocks the worktree: `git worktree unlock {path}`
//! 7. Removes the worktree successfully
//! 8. Cleans up
//!
//! Run with:
//!   cargo run --example 16-worktree-lock
//!
//! Key additions from previous examples:
//!   - Worktree locking to prevent accidental deletion
//!   - Lock status visibility in `git worktree list`
//!   - Safe cleanup pattern: unlock then remove

use anyhow::{Context, Result};
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;
use tracing::info;

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

/// Run a git command, allowing failure (returns Ok with stderr on failure)
fn git_allow_fail(repo_path: &Path, args: &[&str]) -> Result<(bool, String)> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("Failed to run git {:?}", args))?;

    let success = output.status.success();
    let message = if success {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    };

    Ok((success, message))
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

/// List all worktrees (verbose format shows lock status)
fn list_worktrees(repo_path: &Path) -> Result<String> {
    git(repo_path, &["worktree", "list", "-v"])
}

/// Remove a worktree (allows failure to demonstrate lock protection)
fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    git(
        repo_path,
        &["worktree", "remove", &worktree_path.to_string_lossy()],
    )?;
    Ok(())
}

/// Try to remove a worktree, return success status and message
fn try_remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(bool, String)> {
    git_allow_fail(
        repo_path,
        &["worktree", "remove", &worktree_path.to_string_lossy()],
    )
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

fn main() -> Result<()> {
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
        "|                   WORKTREE LOCK EXAMPLE                  |"
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
        "|  Locking worktrees to prevent accidental deletion        |"
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
    // 4. CREATE WORKTREE
    // ============================================================

    let task_id = "bd-xyz789";
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

    // Show initial worktree list
    println!("\n{}", "Worktree list (before lock):".green().bold());
    println!("{}", "-".repeat(70).dimmed());
    let worktrees = list_worktrees(repo_path)?;
    for line in worktrees.lines() {
        println!("  {}", line);
    }
    println!("{}", "-".repeat(70).dimmed());

    // ============================================================
    // 5. LOCK THE WORKTREE
    // ============================================================

    println!("\n{}", "Locking worktree...".yellow().bold());
    let lock_reason = "rstep active";
    lock_worktree(repo_path, &worktree_path, lock_reason)?;
    println!(
        "  {} {} {}",
        "Locked with reason:".green(),
        lock_reason.cyan().bold(),
        "OK".green().bold()
    );

    // Show worktree list with lock status
    println!("\n{}", "Worktree list (after lock):".green().bold());
    println!("{}", "-".repeat(70).dimmed());
    let worktrees = list_worktrees(repo_path)?;
    for line in worktrees.lines() {
        // Highlight locked status
        if line.contains("locked") {
            println!("  {}", line.yellow());
        } else {
            println!("  {}", line);
        }
    }
    println!("{}", "-".repeat(70).dimmed());

    // ============================================================
    // 6. ATTEMPT TO REMOVE LOCKED WORKTREE
    // ============================================================

    println!(
        "\n{}",
        "Attempting to remove locked worktree...".yellow().bold()
    );
    let (success, message) = try_remove_worktree(repo_path, &worktree_path)?;

    if success {
        println!(
            "  {} {}",
            "Result:".dimmed(),
            "UNEXPECTED - worktree was removed!".red().bold()
        );
    } else {
        println!(
            "  {} {}",
            "Result:".dimmed(),
            "FAILED (as expected)".green().bold()
        );
        println!(
            "  {} {}",
            "Error:".dimmed(),
            message.yellow()
        );
    }

    println!(
        "\n{}",
        "This demonstrates that locked worktrees are protected!"
            .blue()
            .bold()
    );

    // Verify worktree still exists
    if worktree_path.exists() {
        println!(
            "  {} {}",
            "Worktree still exists:".green(),
            "YES".green().bold()
        );
    } else {
        anyhow::bail!("Worktree was deleted despite being locked!");
    }

    // ============================================================
    // 7. UNLOCK THE WORKTREE
    // ============================================================

    println!("\n{}", "Unlocking worktree...".yellow().bold());
    unlock_worktree(repo_path, &worktree_path)?;
    println!(
        "  {} {}",
        "Unlocked:".green(),
        "OK".green().bold()
    );

    // Show worktree list after unlock
    println!("\n{}", "Worktree list (after unlock):".green().bold());
    println!("{}", "-".repeat(70).dimmed());
    let worktrees = list_worktrees(repo_path)?;
    for line in worktrees.lines() {
        println!("  {}", line);
    }
    println!("{}", "-".repeat(70).dimmed());

    // ============================================================
    // 8. REMOVE UNLOCKED WORKTREE
    // ============================================================

    println!(
        "\n{}",
        "Removing unlocked worktree...".yellow().bold()
    );
    remove_worktree(repo_path, &worktree_path)?;
    println!(
        "  {} {}",
        "Worktree removed:".green(),
        "YES".green().bold()
    );

    // Delete the branch
    delete_branch(repo_path, &branch_name)?;
    println!(
        "  {} {}",
        "Branch deleted:".green(),
        "YES".green().bold()
    );

    // ============================================================
    // 9. SUMMARY
    // ============================================================

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                  WORKTREE LOCK COMPLETE                  |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );

    println!("\n{}", "Lock lifecycle:".blue().bold());
    println!("{}", "-".repeat(70).dimmed());
    println!(
        "  {} {}",
        "1. CREATE".cyan(),
        "git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}"
    );
    println!(
        "  {} {}",
        "2. LOCK  ".cyan(),
        "git worktree lock {path} --reason \"rstep active\""
    );
    println!(
        "  {} {}",
        "3. WORK  ".cyan(),
        "Worktree is protected from accidental deletion"
    );
    println!(
        "  {} {}",
        "4. VERIFY".cyan(),
        "Run tests in worktree directory"
    );
    println!(
        "  {} {}",
        "5. UNLOCK".cyan(),
        "git worktree unlock {path}"
    );
    println!(
        "  {} {}",
        "6. REMOVE".cyan(),
        "git worktree remove {path}"
    );
    println!(
        "  {} {}",
        "7. DELETE".cyan(),
        "git branch -d rloop/{task-id}"
    );
    println!("{}", "-".repeat(70).dimmed());

    println!("\n{}", "Why lock worktrees?".yellow().bold());
    println!(
        "  {} {}",
        "-".cyan(),
        "Prevents accidental deletion during work"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Protects from 'git worktree prune' cleanup"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Makes parallel execution safer"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Reason field documents why it's locked"
    );

    println!("\n{}", "Lock commands:".yellow().bold());
    println!(
        "  {} {}",
        "Lock:  ".cyan(),
        "git worktree lock <path> --reason \"reason\""
    );
    println!(
        "  {} {}",
        "Unlock:".cyan(),
        "git worktree unlock <path>"
    );
    println!(
        "  {} {}",
        "List:  ".cyan(),
        "git worktree list -v  (shows lock status)"
    );

    info!("{}", "Done.".green());

    // Temp directory auto-deleted on drop
    Ok(())
}
