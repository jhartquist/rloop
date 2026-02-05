//! Git worktree operations for task isolation.
//!
//! This module provides the complete worktree lifecycle:
//!
//! 1. CREATE:  `create_worktree(repo_path, task_id)` - Create isolated branch
//! 2. LOCK:    `lock_worktree(worktree_path, reason)` - Prevent deletion during work
//! 3. WORK:    (Claude operates in worktree)
//! 4. COMMIT:  `commit_changes(worktree_path, message)` - Stage and commit changes
//! 5. VERIFY:  (External: cargo test)
//! 6. MERGE:   `merge_to_main(repo_path, task_id, summary)` - Squash merge to main
//! 7. CLEANUP: `cleanup_worktree(repo_path, task_id)` - Remove worktree and branch

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ============================================================
// CORE GIT COMMAND
// ============================================================

/// Run a git command in the given directory, returning stdout.
///
/// # Errors
///
/// Returns an error if the command fails to execute or exits with non-zero status.
pub fn git(repo_path: &Path, args: &[&str]) -> Result<String> {
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

/// Run a git command, ignoring errors (useful for cleanup operations).
///
/// Returns `Some(stdout)` on success, `None` on failure.
pub fn git_ignore_errors(repo_path: &Path, args: &[&str]) -> Option<String> {
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

// ============================================================
// WORKTREE LIFECYCLE
// ============================================================

/// Phase 1: CREATE - Create a worktree for the given task ID.
///
/// Creates a new worktree at `.rloop/worktrees/{task-id}` on a new branch
/// named `rloop/{task-id}`.
///
/// # Arguments
///
/// * `repo_path` - Path to the main repository
/// * `task_id` - Unique identifier for the task (e.g., "bd-abc123")
///
/// # Returns
///
/// Path to the created worktree.
///
/// # Example
///
/// ```ignore
/// let worktree_path = create_worktree(&repo_path, "bd-fix-bug")?;
/// // worktree_path: {repo_path}/.rloop/worktrees/bd-fix-bug
/// // branch: rloop/bd-fix-bug
/// ```
pub fn create_worktree(repo_path: &Path, task_id: &str) -> Result<PathBuf> {
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

/// Phase 2: LOCK - Lock a worktree with a reason.
///
/// Locks the worktree to prevent accidental deletion by `git worktree prune`
/// or manual removal. The reason is recorded for debugging.
///
/// # Arguments
///
/// * `worktree_path` - Path to the worktree to lock
/// * `reason` - Human-readable reason for the lock (e.g., "rstep active")
///
/// # Example
///
/// ```ignore
/// lock_worktree(&worktree_path, "rstep active")?;
/// ```
pub fn lock_worktree(worktree_path: &Path, reason: &str) -> Result<()> {
    git(
        worktree_path,
        &["worktree", "lock", ".", "--reason", reason],
    )?;
    Ok(())
}

/// Unlock a worktree.
///
/// Must be called before removing the worktree or checking out a different
/// branch in the main repository.
///
/// # Arguments
///
/// * `worktree_path` - Path to the worktree to unlock
pub fn unlock_worktree(worktree_path: &Path) -> Result<()> {
    git(worktree_path, &["worktree", "unlock", "."])?;
    Ok(())
}

/// Phase 4: COMMIT - Commit changes in a worktree.
///
/// Stages all changes and creates a commit. Returns an error if there are
/// no changes to commit.
///
/// # Arguments
///
/// * `worktree_path` - Path to the worktree
/// * `message` - Commit message
///
/// # Returns
///
/// The full commit hash (40 characters).
///
/// # Example
///
/// ```ignore
/// let hash = commit_changes(&worktree_path, "fix: correct add function")?;
/// println!("Committed: {}", &hash[..12]);
/// ```
pub fn commit_changes(worktree_path: &Path, message: &str) -> Result<String> {
    // Stage all changes
    git(worktree_path, &["add", "-A"])?;

    // Check if there are changes to commit
    let status = git(worktree_path, &["status", "--porcelain"])?;
    if status.is_empty() {
        anyhow::bail!("No changes to commit");
    }

    // Commit
    git(worktree_path, &["commit", "-m", message])?;

    // Get commit hash
    let commit_hash = git(worktree_path, &["rev-parse", "HEAD"])?;

    Ok(commit_hash)
}

/// Phase 5: MERGE - Squash merge a task branch to main.
///
/// Checks out main, performs a squash merge of the task branch, and commits
/// with a standardized message format.
///
/// **Important:** The worktree must be unlocked before calling this function,
/// as git checkout requires exclusive access.
///
/// # Arguments
///
/// * `repo_path` - Path to the main repository
/// * `task_id` - Task identifier (used to find branch `rloop/{task-id}`)
/// * `summary` - Short description for the commit message
///
/// # Example
///
/// ```ignore
/// unlock_worktree(&worktree_path)?;
/// merge_to_main(&repo_path, "bd-fix-bug", "fix add function")?;
/// // Creates commit: "task: fix add function"
/// ```
pub fn merge_to_main(repo_path: &Path, task_id: &str, summary: &str) -> Result<()> {
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

/// Phase 6: CLEANUP - Remove worktree and delete branch.
///
/// Performs cleanup after a successful merge:
/// 1. Unlock worktree (ignores errors if not locked)
/// 2. Remove worktree directory
/// 3. Force delete branch (required after squash merge)
///
/// # Arguments
///
/// * `repo_path` - Path to the main repository
/// * `task_id` - Task identifier
///
/// # Returns
///
/// A tuple `(worktree_removed, branch_deleted)` indicating success of each operation.
///
/// # Example
///
/// ```ignore
/// let (wt_ok, br_ok) = cleanup_worktree(&repo_path, "bd-fix-bug")?;
/// assert!(wt_ok && br_ok, "Cleanup failed");
/// ```
pub fn cleanup_worktree(repo_path: &Path, task_id: &str) -> Result<(bool, bool)> {
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

    // 3. Delete branch with -D (force delete after squash merge)
    //    Note: -d would fail because git doesn't recognize squash-merged branches
    let branch_deleted = git(repo_path, &["branch", "-D", &branch_name]).is_ok();

    Ok((worktree_removed, branch_deleted))
}

// ============================================================
// QUERY FUNCTIONS
// ============================================================

/// Get the current branch name.
pub fn current_branch(repo_path: &Path) -> Result<String> {
    git(repo_path, &["branch", "--show-current"])
}

/// Get the last commit hash (short form, 7 characters).
pub fn last_commit(repo_path: &Path) -> Result<String> {
    git(repo_path, &["rev-parse", "--short", "HEAD"])
}

/// Get the last commit hash (full form, 40 characters).
pub fn last_commit_full(repo_path: &Path) -> Result<String> {
    git(repo_path, &["rev-parse", "HEAD"])
}

/// Get git log in oneline format.
///
/// # Arguments
///
/// * `repo_path` - Path to the repository
/// * `limit` - Maximum number of commits to show
pub fn get_log(repo_path: &Path, limit: usize) -> Result<String> {
    git(repo_path, &["log", "--oneline", &format!("-{}", limit)])
}

/// List all worktrees.
pub fn list_worktrees(repo_path: &Path) -> Result<String> {
    git(repo_path, &["worktree", "list"])
}

/// List all worktrees with verbose output (shows lock status).
pub fn list_worktrees_verbose(repo_path: &Path) -> Result<String> {
    git(repo_path, &["worktree", "list", "-v"])
}

/// List all branches.
pub fn list_branches(repo_path: &Path) -> Result<String> {
    git(repo_path, &["branch", "-a"])
}

/// Get the diff between two commits or refs.
pub fn get_diff(repo_path: &Path, from: &str, to: &str) -> Result<String> {
    git(repo_path, &["diff", from, to])
}

// ============================================================
// INITIALIZATION (for tests)
// ============================================================

/// Initialize a git repo with an initial commit.
///
/// Useful for testing. Creates a repo with test user config and commits
/// all existing files.
pub fn init_repo(repo_path: &Path) -> Result<()> {
    git(repo_path, &["init"])?;
    git(repo_path, &["config", "user.email", "test@example.com"])?;
    git(repo_path, &["config", "user.name", "Test User"])?;
    git(repo_path, &["add", "-A"])?;
    git(repo_path, &["commit", "-m", "Initial commit"])?;
    Ok(())
}

// ============================================================
// HELPERS
// ============================================================

/// Get the worktree path for a task ID.
///
/// Returns the path where the worktree would be located, without checking
/// if it exists.
pub fn worktree_path(repo_path: &Path, task_id: &str) -> PathBuf {
    repo_path.join(".rloop/worktrees").join(task_id)
}

/// Get the branch name for a task ID.
pub fn branch_name(task_id: &str) -> String {
    format!("rloop/{}", task_id)
}
