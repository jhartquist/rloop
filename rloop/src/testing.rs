use std::path::Path;
use std::sync::Mutex;

use crate::config::Config;
use crate::step::{StepExecutor, StepResult};
use crate::task::Task;

/// Executor that wraps a real executor, recording results for later playback.
pub struct RecordingExecutor<E> {
    inner: E,
    results: Mutex<Vec<StepResult>>,
}

impl<E: StepExecutor> RecordingExecutor<E> {
    pub fn new(inner: E) -> Self {
        Self {
            inner,
            results: Mutex::new(Vec::new()),
        }
    }

    /// Save all recorded results to a JSON file.
    ///
    /// # Errors
    /// Returns an error if serialization or file writing fails.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let results = self.results.lock().expect("results lock poisoned");
        let json = serde_json::to_string_pretty(&*results)?;
        std::fs::write(path.as_ref(), json)?;
        Ok(())
    }
}

impl<E: StepExecutor> StepExecutor for RecordingExecutor<E> {
    async fn execute(
        &self,
        task: &Task,
        worktree_path: &Path,
        config: &Config,
        prior_learnings: &[String],
    ) -> anyhow::Result<StepResult> {
        let result = self
            .inner
            .execute(task, worktree_path, config, prior_learnings)
            .await?;
        self.results
            .lock()
            .expect("results lock poisoned")
            .push(result.clone());
        Ok(result)
    }
}

/// Executor that replays pre-recorded results from a fixture file.
pub struct PlaybackExecutor {
    results: Mutex<Vec<StepResult>>,
}

impl PlaybackExecutor {
    /// Load recorded results from a JSON file.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or contains invalid JSON.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path.as_ref())?;
        let results: Vec<StepResult> = serde_json::from_str(&json)?;
        // Reverse so we can pop from the end efficiently
        let mut results = results;
        results.reverse();
        Ok(Self {
            results: Mutex::new(results),
        })
    }
}

impl StepExecutor for PlaybackExecutor {
    async fn execute(
        &self,
        _task: &Task,
        _worktree_path: &Path,
        _config: &Config,
        _prior_learnings: &[String],
    ) -> anyhow::Result<StepResult> {
        self.results
            .lock()
            .expect("results lock poisoned")
            .pop()
            .ok_or_else(|| anyhow::anyhow!("playback exhausted: no more recorded results"))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::Model;
    use crate::step::StepOutcome;
    use crate::task::TaskId;

    fn sample_result(id: u32) -> StepResult {
        StepResult {
            task_id: TaskId(id),
            outcome: StepOutcome::Success {
                summary: format!("task {id} done"),
            },
            model: Model::Haiku,
            cost_usd: 0.01,
            duration: Duration::from_secs(5),
            cc_session_id: Some(format!("session-{id}")),
            events: vec![],
        }
    }

    #[test]
    fn roundtrip_step_result_json() {
        let result = sample_result(0);
        let json = serde_json::to_string_pretty(&result).unwrap();
        let loaded: StepResult = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.task_id, TaskId(0));
        assert!(matches!(loaded.outcome, StepOutcome::Success { .. }));
    }

    #[test]
    fn playback_returns_results_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.json");

        let results = vec![sample_result(0), sample_result(1)];
        let json = serde_json::to_string_pretty(&results).unwrap();
        std::fs::write(&path, json).unwrap();

        let executor = PlaybackExecutor::load(&path).unwrap();

        // Use block_on since we're in a sync test
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dummy_task = crate::task::Task {
            id: TaskId(0),
            model: Model::Haiku,
            depends_on: vec![],
            verification: None,
            completed: false,
            description: String::new(),
            source_path: std::path::PathBuf::new(),
        };
        let config = Config::default();

        let r0 = rt
            .block_on(executor.execute(&dummy_task, Path::new("/tmp"), &config, &[]))
            .unwrap();
        assert_eq!(r0.task_id, TaskId(0));

        let r1 = rt
            .block_on(executor.execute(&dummy_task, Path::new("/tmp"), &config, &[]))
            .unwrap();
        assert_eq!(r1.task_id, TaskId(1));

        let r2 = rt.block_on(executor.execute(&dummy_task, Path::new("/tmp"), &config, &[]));
        assert!(r2.is_err());
    }

    /// Verify `PlaybackExecutor`'s `Mutex`-backed results are safe under concurrent access
    /// from multiple threads all popping from the same shared queue.
    #[test]
    fn playback_concurrent_access() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.json");

        // Create 20 results
        let results: Vec<StepResult> = (0..20).map(sample_result).collect();
        let json = serde_json::to_string_pretty(&results).unwrap();
        std::fs::write(&path, json).unwrap();

        let executor = Arc::new(PlaybackExecutor::load(&path).unwrap());
        let collected: Arc<Mutex<Vec<TaskId>>> = Arc::default();
        let mut handles = Vec::new();

        // Spawn 20 threads, each consuming one result
        for _ in 0..20 {
            let ex = Arc::clone(&executor);
            let ids = Arc::clone(&collected);
            handles.push(std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let dummy_task = crate::task::Task {
                    id: TaskId(0),
                    model: Model::Haiku,
                    depends_on: vec![],
                    verification: None,
                    completed: false,
                    description: String::new(),
                    source_path: std::path::PathBuf::new(),
                };
                let config = Config::default();
                if let Ok(result) =
                    rt.block_on(ex.execute(&dummy_task, Path::new("/tmp"), &config, &[]))
                {
                    ids.lock().unwrap().push(result.task_id);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let ids = collected.lock().unwrap();
        assert_eq!(ids.len(), 20, "all 20 results should be consumed");

        // Each task ID 0..20 should appear exactly once
        let mut sorted: Vec<u32> = ids.iter().map(|t| t.0).collect();
        sorted.sort_unstable();
        let expected: Vec<u32> = (0..20).collect();
        assert_eq!(sorted, expected);
    }
}
