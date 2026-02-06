use std::fmt;
use std::path::Path;

use serde::Serialize;

use crate::config::Config;
use crate::task::TaskId;

/// Unique identifier for a session (one invocation of rloop).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct SessionId(pub String);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Result of a complete session (one invocation of rloop).
///
/// A session continues past individual task failures — unblocked tasks in
/// the DAG still execute. Only tasks transitively blocked by a failure are skipped.
pub struct SessionResult {
    pub session_id: SessionId,
    pub branch: String,
    pub completed_tasks: Vec<TaskId>,
    pub failed_tasks: Vec<TaskId>,
    pub skipped_tasks: Vec<TaskId>,
    pub auto_merged: bool,
}

/// Run a session: resolve DAG, execute tasks in order, merge results.
#[allow(clippy::unused_async)] // will await execute_step in Phase 3
pub async fn run_session(
    _repo: &Path,
    _target_task: TaskId,
    _config: &Config,
    _auto_merge: bool,
) -> anyhow::Result<SessionResult> {
    todo!()
}
