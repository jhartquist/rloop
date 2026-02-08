use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::task::TaskId;

/// An active git worktree for task execution.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub task_id: TaskId,
}

/// Run a git command in the given directory, returning trimmed stdout on success.
///
/// # Errors
/// Returns an error if the git command fails or returns a non-zero exit code.
pub fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
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

/// Check if a branch exists.
///
/// # Errors
/// Returns an error if the git command fails (e.g. not a repo).
pub fn branch_exists(repo: impl AsRef<Path>, branch: &str) -> anyhow::Result<bool> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", branch])
        .current_dir(repo.as_ref())
        .output()
        .context("failed to run git rev-parse")?;
    Ok(output.status.success())
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
/// Returns a clear error if the worktree branch already exists from a previous session.
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

    // Check for stale worktree branch from a previous session
    if branch_exists(repo, &branch)? {
        bail!(
            "stale worktree branch '{branch}' exists from a previous run. Delete it with: git branch -D {branch}"
        );
    }

    // Check for stale worktree directory
    if wt_path.exists() {
        bail!(
            "stale worktree directory '{}' exists from a previous run. Remove it with: rm -rf {}",
            wt_path.display(),
            wt_path.display()
        );
    }

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

/// Rebase `branch` onto `onto`. Both must be valid refs.
///
/// Runs `git rebase <onto> <branch>` from the repo root.
/// If the rebase has no conflicts, this is a no-op when `branch` is already
/// a descendant of `onto`.
///
/// # Errors
/// Returns an error if the rebase fails (typically a merge conflict).
pub fn rebase(repo: impl AsRef<Path>, branch: &str, onto: &str) -> anyhow::Result<()> {
    git(repo.as_ref(), &["rebase", onto, branch])
        .with_context(|| format!("failed to rebase {branch} onto {onto}"))?;
    Ok(())
}

/// Abort an in-progress rebase.
///
/// # Errors
/// Returns an error if there is no rebase in progress or the abort fails.
pub fn rebase_abort(repo: impl AsRef<Path>) -> anyhow::Result<()> {
    git(repo.as_ref(), &["rebase", "--abort"]).context("failed to abort rebase")?;
    Ok(())
}

/// Start a merge of `branch` into the current HEAD without committing.
///
/// Returns `true` if the merge is clean (no conflicts), `false` if conflicts remain.
/// On conflict, the working tree has conflict markers that need manual resolution.
///
/// # Errors
/// Returns an error if the merge command fails for reasons other than conflicts
/// (e.g. invalid branch name, not a git repo).
pub fn merge_no_commit(repo: impl AsRef<Path>, branch: &str) -> anyhow::Result<bool> {
    let repo = repo.as_ref();
    let output = Command::new("git")
        .args(["merge", branch, "--no-commit", "--no-ff"])
        .current_dir(repo)
        .output()
        .context("failed to run git merge")?;
    if output.status.success() {
        return Ok(true);
    }
    // Exit code 1 with "CONFLICT" in output means merge conflicts
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains("CONFLICT") || stderr.contains("CONFLICT") {
        return Ok(false);
    }
    bail!(
        "git merge --no-commit failed: {}{}",
        stdout.trim(),
        stderr.trim()
    );
}

/// Abort an in-progress merge.
///
/// # Errors
/// Returns an error if there is no merge in progress or the abort fails.
pub fn merge_abort(repo: impl AsRef<Path>) -> anyhow::Result<()> {
    git(repo.as_ref(), &["merge", "--abort"]).context("failed to abort merge")?;
    Ok(())
}

/// Check whether the working tree contains conflict markers.
///
/// Runs `git diff --check` which detects `<<<<<<<` markers left by a merge.
/// Returns `true` if conflict markers remain, `false` if clean.
///
/// # Errors
/// Returns an error if the git command cannot be executed.
pub fn has_conflict_markers(repo: impl AsRef<Path>) -> anyhow::Result<bool> {
    let repo = repo.as_ref();
    let output = Command::new("git")
        .args(["diff", "--check"])
        .current_dir(repo)
        .output()
        .context("failed to run git diff --check")?;
    // Exit code 0 = clean, non-zero = conflicts or whitespace issues
    // Check stdout for "conflict" marker indicators
    if output.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains("conflict") || stdout.contains("<<<<<<<"))
}

/// Clean up a worktree: unlock (if locked), remove, and delete the branch.
///
/// # Errors
/// Returns an error if worktree removal or branch deletion commands fail.
pub fn cleanup_worktree(repo: impl AsRef<Path>, worktree: &Worktree) -> anyhow::Result<()> {
    cleanup_worktree_impl(repo.as_ref(), worktree, false)
}

/// Clean up a worktree but preserve its branch (skip `git branch -D`).
///
/// Used when a task's merge fails — the branch is kept for manual recovery
/// or a future merge-fix agent.
///
/// # Errors
/// Returns an error if worktree removal fails.
pub fn cleanup_worktree_preserve_branch(
    repo: impl AsRef<Path>,
    worktree: &Worktree,
) -> anyhow::Result<()> {
    cleanup_worktree_impl(repo.as_ref(), worktree, true)
}

fn cleanup_worktree_impl(
    repo: &Path,
    worktree: &Worktree,
    preserve_branch: bool,
) -> anyhow::Result<()> {
    let wt_path_str = worktree.path.to_str().context("non-UTF-8 worktree path")?;

    // Best-effort unlock — worktree may not be locked
    if let Err(e) = git(repo, &["worktree", "unlock", wt_path_str]) {
        eprintln!("warning: failed to unlock worktree (may not be locked): {e}");
    }

    git(repo, &["worktree", "remove", wt_path_str, "--force"])
        .with_context(|| format!("failed to remove worktree for task {}", worktree.task_id))?;

    if !preserve_branch {
        git(repo, &["branch", "-D", &worktree.branch])
            .with_context(|| format!("failed to delete branch {}", worktree.branch))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a temp git repo with an initial commit.
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        git(&repo, &["init"]).unwrap();
        git(&repo, &["config", "user.email", "test@test.com"]).unwrap();
        git(&repo, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(repo.join("README.md"), "# test\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "initial"]).unwrap();
        (dir, repo)
    }

    // --- branch_exists tests ---

    #[test]
    fn branch_exists_returns_true_for_existing_branch() {
        let (_dir, repo) = init_repo();

        // main branch should exist
        assert!(branch_exists(&repo, "main").unwrap());

        // Create a new branch
        git(&repo, &["branch", "test-branch"]).unwrap();
        assert!(branch_exists(&repo, "test-branch").unwrap());
    }

    #[test]
    fn branch_exists_returns_false_for_nonexistent_branch() {
        let (_dir, repo) = init_repo();

        assert!(!branch_exists(&repo, "nonexistent-branch").unwrap());
    }

    // --- create_worktree stale branch detection tests ---

    #[test]
    fn create_worktree_fails_on_stale_branch() {
        let (_dir, repo) = init_repo();

        // Create a stale worktree branch
        git(&repo, &["branch", "rloop-wt-05"]).unwrap();

        // Attempt to create worktree should fail with clear error
        let result = create_worktree(&repo, TaskId(5), "main");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("stale worktree branch"));
        assert!(err.contains("rloop-wt-05"));
        assert!(err.contains("git branch -D"));
    }

    #[test]
    fn create_worktree_fails_on_stale_directory() {
        let (_dir, repo) = init_repo();

        // Create a stale worktree directory
        let wt_path = repo.join(".rloop/worktrees/05");
        std::fs::create_dir_all(&wt_path).unwrap();
        std::fs::write(wt_path.join("stale.txt"), "stale file\n").unwrap();

        // Attempt to create worktree should fail with clear error
        let result = create_worktree(&repo, TaskId(5), "main");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("stale worktree directory"));
        assert!(err.contains("rm -rf"));
    }

    #[test]
    fn create_worktree_succeeds_when_clean() {
        let (_dir, repo) = init_repo();

        let worktree = create_worktree(&repo, TaskId(5), "main").unwrap();
        assert_eq!(worktree.task_id, TaskId(5));
        assert_eq!(worktree.branch, "rloop-wt-05");
        assert!(worktree.path.exists());

        // Cleanup
        cleanup_worktree(&repo, &worktree).unwrap();
    }

    // --- merge_no_commit tests ---

    #[test]
    fn merge_no_commit_clean() {
        let (_dir, repo) = init_repo();

        // Create a branch with a new file
        git(&repo, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "feature content\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "add feature"]).unwrap();
        git(&repo, &["checkout", "main"]).unwrap();

        // Merge should be clean
        let clean = merge_no_commit(&repo, "feature").unwrap();
        assert!(clean, "merge should be clean when there are no conflicts");

        // Abort to clean up
        git(&repo, &["merge", "--abort"]).ok(); // May not be in merge state if clean
    }

    #[test]
    fn merge_no_commit_with_conflict() {
        let (_dir, repo) = init_repo();

        // Create conflicting changes on two branches
        git(&repo, &["checkout", "-b", "branch-a"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "content A\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "write A"]).unwrap();

        git(&repo, &["checkout", "main"]).unwrap();
        git(&repo, &["checkout", "-b", "branch-b"]).unwrap();
        std::fs::write(repo.join("shared.txt"), "content B\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "write B"]).unwrap();

        // Merge branch-a into branch-b (current) should conflict
        let clean = merge_no_commit(&repo, "branch-a").unwrap();
        assert!(!clean, "merge should have conflicts");

        // Clean up
        merge_abort(&repo).unwrap();
    }

    // --- merge_abort tests ---

    #[test]
    fn merge_abort_after_conflict() {
        let (_dir, repo) = init_repo();

        git(&repo, &["checkout", "-b", "branch-a"]).unwrap();
        std::fs::write(repo.join("file.txt"), "A\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "write A"]).unwrap();

        git(&repo, &["checkout", "main"]).unwrap();
        git(&repo, &["checkout", "-b", "branch-b"]).unwrap();
        std::fs::write(repo.join("file.txt"), "B\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "write B"]).unwrap();

        merge_no_commit(&repo, "branch-a").unwrap();
        merge_abort(&repo).unwrap();

        // Working tree should be clean after abort
        let status = git(&repo, &["status", "--porcelain"]).unwrap();
        assert!(
            status.is_empty(),
            "working tree should be clean after abort"
        );
    }

    // --- has_conflict_markers tests ---

    #[test]
    fn has_conflict_markers_clean() {
        let (_dir, repo) = init_repo();

        // Clean repo should have no conflict markers
        let has = has_conflict_markers(&repo).unwrap();
        assert!(!has, "clean repo should have no conflict markers");
    }

    #[test]
    fn has_conflict_markers_with_markers() {
        let (_dir, repo) = init_repo();

        // Create a conflict
        git(&repo, &["checkout", "-b", "branch-a"]).unwrap();
        std::fs::write(repo.join("file.txt"), "A\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "A"]).unwrap();

        git(&repo, &["checkout", "main"]).unwrap();
        git(&repo, &["checkout", "-b", "branch-b"]).unwrap();
        std::fs::write(repo.join("file.txt"), "B\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "B"]).unwrap();

        merge_no_commit(&repo, "branch-a").unwrap();

        // Should detect conflict markers
        let has = has_conflict_markers(&repo).unwrap();
        assert!(has, "should detect conflict markers");

        merge_abort(&repo).unwrap();
    }

    #[test]
    fn has_conflict_markers_after_resolution() {
        let (_dir, repo) = init_repo();

        // Create a conflict
        git(&repo, &["checkout", "-b", "branch-a"]).unwrap();
        std::fs::write(repo.join("file.txt"), "A\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "A"]).unwrap();

        git(&repo, &["checkout", "main"]).unwrap();
        git(&repo, &["checkout", "-b", "branch-b"]).unwrap();
        std::fs::write(repo.join("file.txt"), "B\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "B"]).unwrap();

        merge_no_commit(&repo, "branch-a").unwrap();

        // Resolve the conflict manually
        std::fs::write(repo.join("file.txt"), "resolved\n").unwrap();
        git(&repo, &["add", "file.txt"]).unwrap();

        // Should no longer detect conflict markers
        let has = has_conflict_markers(&repo).unwrap();
        assert!(!has, "should not detect conflict markers after resolution");

        // Commit the merge
        git(&repo, &["commit", "--no-edit"]).unwrap();
    }
}
