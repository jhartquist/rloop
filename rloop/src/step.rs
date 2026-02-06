use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::model::Model;
use crate::task::{Task, TaskId};

/// Outcome of a single step (one execution attempt of a task).
pub enum StepOutcome {
    Success { summary: String },
    Failed { reason: String, learnings: String },
}

/// Result of a single step execution.
pub struct StepResult {
    pub task_id: TaskId,
    pub outcome: StepOutcome,
    pub model: Model,
    pub cost_usd: f64,
    pub duration: Duration,
    /// Claude Code session ID — for tracing back to CC logs or future session resume.
    pub cc_session_id: Option<String>,
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
