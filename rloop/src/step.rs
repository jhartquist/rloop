use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::task::Task;

/// Outcome of a single step (one execution attempt of a task).
pub enum StepOutcome {
    Success { summary: String },
    Failed { reason: String, learnings: String },
}

/// Result of executing a task (may include multiple retry attempts).
pub struct StepResult {
    pub task_id: String,
    pub outcome: StepOutcome,
    pub attempts: u32,
    pub duration: Duration,
}

/// Execute a task in a worktree: run the agent, verify, retry on failure.
#[allow(clippy::unused_async)] // will await SDK calls in Phase 3
pub async fn execute_step(
    _task: &Task,
    _worktree_path: &Path,
    _config: &Config,
    _prior_learnings: &[String],
) -> anyhow::Result<StepResult> {
    todo!()
}
