use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::task::TaskId;

/// An active git worktree for task execution.
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub task_id: TaskId,
}

/// Run a git command in the given directory, returning trimmed stdout on success.
pub(crate) fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {stderr}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Return the current branch name.
///
/// # Errors
/// Returns an error if the git command fails (e.g. detached HEAD, not a repo).
pub fn current_branch(repo: impl AsRef<Path>) -> anyhow::Result<String> {
    git(repo.as_ref(), &["branch", "--show-current"])
}

/// Assert the repo has at least one commit.
///
/// # Errors
/// Returns an error if the repo has no commits.
pub fn assert_has_commits(repo: impl AsRef<Path>) -> anyhow::Result<()> {
    git(repo.as_ref(), &["rev-parse", "HEAD"])
        .context("repository has no commits — run `rloop init` or make an initial commit")?;
    Ok(())
}

/// Assert the working tree has no uncommitted changes.
///
/// # Errors
/// Returns an error if there are staged or unstaged changes.
pub fn assert_clean_worktree(repo: impl AsRef<Path>) -> anyhow::Result<()> {
    let status = git(repo.as_ref(), &["status", "--porcelain"])?;
    if !status.is_empty() {
        anyhow::bail!("uncommitted changes in working tree — commit or stash before running rloop");
    }
    Ok(())
}

/// Create a new worktree for a task, branched off `base_branch`.
///
/// The worktree lives at `{repo}/.rloop/worktrees/{task_id:02}/` on a new
/// branch named `rloop-wt-{task_id:02}`.
///
/// # Errors
/// Returns an error if directory creation fails or git worktree command fails.
pub fn create_worktree(
    repo: impl AsRef<Path>,
    task_id: TaskId,
    base_branch: &str,
) -> anyhow::Result<Worktree> {
    let repo = repo.as_ref();
    let label = format!("{task_id:02}");
    let worktree_dir = repo.join(".rloop").join("worktrees");
    std::fs::create_dir_all(&worktree_dir)
        .with_context(|| format!("failed to create {}", worktree_dir.display()))?;

    let wt_path = worktree_dir.join(&label);
    let branch = format!("rloop-wt-{label}");

    git(
        repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().context("non-UTF-8 worktree path")?,
            "-b",
            &branch,
            base_branch,
        ],
    )
    .with_context(|| format!("failed to create worktree for task {task_id}"))?;

    Ok(Worktree {
        path: wt_path,
        branch,
        task_id,
    })
}

/// Lock a worktree to prevent accidental removal.
///
/// # Errors
/// Returns an error if the git worktree lock command fails.
pub fn lock_worktree(worktree: &Worktree) -> anyhow::Result<()> {
    git(
        &worktree.path,
        &[
            "worktree",
            "lock",
            worktree.path.to_str().context("non-UTF-8 worktree path")?,
        ],
    )
    .with_context(|| format!("failed to lock worktree for task {}", worktree.task_id))?;
    Ok(())
}

/// Unlock a previously locked worktree.
///
/// # Errors
/// Returns an error if the git worktree unlock command fails.
pub fn unlock_worktree(worktree: &Worktree) -> anyhow::Result<()> {
    git(
        &worktree.path,
        &[
            "worktree",
            "unlock",
            worktree.path.to_str().context("non-UTF-8 worktree path")?,
        ],
    )
    .with_context(|| format!("failed to unlock worktree for task {}", worktree.task_id))?;
    Ok(())
}

/// Move a branch ref forward to point at `to_ref` (fast-forward).
///
/// Uses `git branch -f` to update the ref without checkout.
///
/// # Errors
/// Returns an error if the git branch command fails.
pub fn fast_forward_ref(repo: impl AsRef<Path>, branch: &str, to_ref: &str) -> anyhow::Result<()> {
    git(repo.as_ref(), &["branch", "-f", branch, to_ref])
        .with_context(|| format!("failed to fast-forward {branch} to {to_ref}"))?;
    Ok(())
}

/// Clean up a worktree: unlock (if locked), remove, and delete the branch.
///
/// # Errors
/// Returns an error if worktree removal or branch deletion commands fail.
pub fn cleanup_worktree(repo: impl AsRef<Path>, worktree: &Worktree) -> anyhow::Result<()> {
    let repo = repo.as_ref();
    // Try unlocking — ignore errors since it may not be locked
    let wt_path_str = worktree.path.to_str().context("non-UTF-8 worktree path")?;

    let _ = git(repo, &["worktree", "unlock", wt_path_str]);

    git(repo, &["worktree", "remove", wt_path_str, "--force"])
        .with_context(|| format!("failed to remove worktree for task {}", worktree.task_id))?;

    git(repo, &["branch", "-D", &worktree.branch])
        .with_context(|| format!("failed to delete branch {}", worktree.branch))?;

    Ok(())
}
