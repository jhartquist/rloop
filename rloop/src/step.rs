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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_outcome_variants() {
        let success = StepOutcome::Success {
            summary: "did the thing".into(),
        };
        assert!(matches!(success, StepOutcome::Success { summary } if summary == "did the thing"));

        let failed = StepOutcome::Failed {
            reason: "tests failed".into(),
            learnings: "need to add dep".into(),
        };
        assert!(matches!(failed, StepOutcome::Failed { reason, .. } if reason == "tests failed"));
    }

    #[test]
    fn step_result_fields() {
        let result = StepResult {
            task_id: TaskId(5),
            outcome: StepOutcome::Success { summary: "ok".into() },
            model: Model::Sonnet,
            cost_usd: 0.05,
            duration: Duration::from_secs(30),
            cc_session_id: None,
        };
        assert_eq!(result.task_id, TaskId(5));
        assert!(matches!(result.model, Model::Sonnet));
        assert!(result.cc_session_id.is_none());

        let result_with_session = StepResult {
            task_id: TaskId(3),
            outcome: StepOutcome::Failed {
                reason: "fail".into(),
                learnings: "learn".into(),
            },
            model: Model::Opus,
            cost_usd: 0.10,
            duration: Duration::from_secs(60),
            cc_session_id: Some("session-abc".into()),
        };
        assert_eq!(result_with_session.cc_session_id.as_deref(), Some("session-abc"));
    }
}
