use std::path::{Path, PathBuf};

use crate::task::TaskId;

/// An active git worktree for task execution.
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub task_id: TaskId,
}

/// Create a new worktree for a task, branched off `base_branch`.
pub fn create_worktree(
    _repo: &Path,
    _task_id: TaskId,
    _base_branch: &str,
) -> anyhow::Result<Worktree> {
    todo!()
}

/// Lock a worktree to prevent accidental removal.
pub fn lock_worktree(_worktree: &Worktree) -> anyhow::Result<()> {
    todo!()
}

/// Unlock a previously locked worktree.
pub fn unlock_worktree(_worktree: &Worktree) -> anyhow::Result<()> {
    todo!()
}

/// Merge a worktree's branch into the target branch.
pub fn merge_worktree(
    _repo: &Path,
    _worktree: &Worktree,
    _into_branch: &str,
) -> anyhow::Result<()> {
    todo!()
}

/// Clean up a worktree: unlock, remove, delete branch.
pub fn cleanup_worktree(_repo: &Path, _worktree: &Worktree) -> anyhow::Result<()> {
    todo!()
}
