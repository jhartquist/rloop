use std::path::Path;

use crate::config::Config;

/// Result of a complete session (one invocation of rloop).
pub struct SessionResult {
    pub branch: String,
    pub completed_tasks: Vec<String>,
    pub failed_task: Option<String>,
    pub auto_merged: bool,
}

/// Run a session: resolve DAG, execute tasks in order, merge results.
#[allow(clippy::unused_async)] // will await execute_step in Phase 3
pub async fn run_session(
    _repo: &Path,
    _target_task: &str,
    _config: &Config,
    _auto_merge: bool,
) -> anyhow::Result<SessionResult> {
    todo!()
}
