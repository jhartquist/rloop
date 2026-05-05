mod helpers;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rloop::config::Config;
use rloop::log::{Event, load_events};
use rloop::model::Model;
use rloop::session::{run_all_session_with, run_session_with};
use rloop::step::{StepExecutor, StepOutcome, StepResult};
use rloop::task::{Task, TaskId};

fn default_config() -> Config {
    Config {
        model: Model::Haiku,
        max_turns: 50,
        max_retries: 3,
        timeout_seconds: 300,
        ..Config::default()
    }
}

struct MockExecutor {
    fail_tasks: HashSet<TaskId>,
}

impl MockExecutor {
    fn all_succeed() -> Self {
        Self {
            fail_tasks: HashSet::new(),
        }
    }

    fn failing(fail_ids: &[u32]) -> Self {
        Self {
            fail_tasks: fail_ids.iter().map(|&id| TaskId(id)).collect(),
        }
    }
}

impl StepExecutor for MockExecutor {
    async fn execute(
        &self,
        task: &Task,
        _worktree_path: &std::path::Path,
        _config: &Config,
        _prior_learnings: &[String],
        _attempt: u32,
    ) -> anyhow::Result<StepResult> {
        let outcome = if self.fail_tasks.contains(&task.id) {
            StepOutcome::Failed {
                reason: "mock failure".into(),
                learnings: "mock learning".into(),
            }
        } else {
            StepOutcome::Success {
                summary: "mock success".into(),
            }
        };
        Ok(StepResult {
            task_id: task.id,
            outcome,
            model: Model::Haiku,
            cost_usd: 0.01,
            duration: Duration::from_secs(10),
            input_tokens: 5000,
            output_tokens: 2000,
            cache_read_tokens: 1000,
            cache_creation_tokens: 500,
            model_id: Some("claude-haiku-4-5-20251001".into()),
            cc_session_id: Some("mock-session-id".into()),
            events: vec![],
        })
    }
}

#[tokio::test]
async fn single_task_no_deps() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Do something"));
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(0),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert!(result.completed_tasks.contains(&TaskId(0)));
    assert!(result.failed_tasks.is_empty());
    // Main working tree stays on original branch
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

#[tokio::test]
async fn linear_chain() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "First"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Second"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[1], None, false, "Third"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(2),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert_eq!(result.completed_tasks.len(), 3);
}

#[tokio::test]
async fn skip_completed_deps() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(
        &tasks_dir,
        0,
        &helpers::full_task(0, "Sonnet", &[], None, true, "Already done"),
    );
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Run this"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(1),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert!(!result.completed_tasks.contains(&TaskId(0)));
    assert!(result.completed_tasks.contains(&TaskId(1)));
}

#[tokio::test]
async fn failure_blocks_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Base"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[1], None, false, "Blocked by 1"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::failing(&[1]);
    let result = run_session_with(
        &repo,
        TaskId(2),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert!(result.completed_tasks.contains(&TaskId(0)));
    assert!(result.failed_tasks.contains(&TaskId(1)));
    assert!(result.skipped_tasks.contains(&TaskId(2)));
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

#[tokio::test]
async fn failure_continues_unblocked() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Diamond: 0 is root, 1 fails, 2 succeeds, 3 depends on both
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Should succeed"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[1, 2], None, false, "Blocked by 1"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::failing(&[1]);
    let result = run_session_with(
        &repo,
        TaskId(3),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert!(result.completed_tasks.contains(&TaskId(0)));
    assert!(result.failed_tasks.contains(&TaskId(1)));
    assert!(result.completed_tasks.contains(&TaskId(2)));
    assert!(result.skipped_tasks.contains(&TaskId(3)));
}

#[tokio::test]
async fn session_branch_naming() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 5, &helpers::minimal_task(5, "Target task"));
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(5),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert_eq!(result.branch, "rloop-05");
    // Main working tree untouched
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

#[tokio::test]
async fn auto_merge_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(0),
        &default_config(),
        true,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert!(result.auto_merged);
}

#[tokio::test]
async fn auto_merge_skipped_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Will fail"));
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::failing(&[0]);
    let result = run_session_with(
        &repo,
        TaskId(0),
        &default_config(),
        true,
        Arc::new(executor),
    )
    .await
    .unwrap();
    assert!(!result.auto_merged);
    assert!(result.failed_tasks.contains(&TaskId(0)));
}

#[tokio::test]
async fn session_logs_events() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    let sessions_dir = repo.join(".rloop").join("sessions");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let _result = run_session_with(
        &repo,
        TaskId(0),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(!entries.is_empty(), "Session should produce a log file");
}

#[tokio::test]
async fn run_all_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "First"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Second"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[1], None, false, "Third"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_all_session_with(&repo, &default_config(), false, Arc::new(executor))
        .await
        .unwrap();
    assert_eq!(result.completed_tasks.len(), 3);
    assert!(result.branch.contains("rloop-all-"));
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

#[tokio::test]
async fn run_all_skips_completed() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(
        &tasks_dir,
        0,
        &helpers::full_task(0, "Sonnet", &[], None, true, "Already done"),
    );
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Run this"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_all_session_with(&repo, &default_config(), false, Arc::new(executor))
        .await
        .unwrap();
    assert!(!result.completed_tasks.contains(&TaskId(0)));
    assert!(result.completed_tasks.contains(&TaskId(1)));
}

#[tokio::test]
async fn run_all_nothing_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(
        &tasks_dir,
        0,
        &helpers::full_task(0, "Sonnet", &[], None, true, "Done"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_all_session_with(&repo, &default_config(), false, Arc::new(executor))
        .await
        .unwrap();
    assert!(result.completed_tasks.is_empty());
}

#[tokio::test]
async fn session_fails_on_no_commits() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let tasks_dir = dir.path().join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));

    let executor = MockExecutor::all_succeed();
    let err = run_session_with(
        dir.path(),
        TaskId(0),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("no commits"),
        "expected no-commits error, got: {msg}"
    );
}

#[tokio::test]
async fn session_fails_on_dirty_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));
    helpers::commit_all(&repo, "add tasks");

    // Dirty the working tree
    std::fs::write(repo.join("dirty.txt"), "uncommitted\n").unwrap();

    let executor = MockExecutor::all_succeed();
    let err = run_session_with(
        &repo,
        TaskId(0),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("uncommitted"),
        "expected dirty-tree error, got: {msg}"
    );
}

#[tokio::test]
async fn metrics_accumulate_for_single_task() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(0),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();

    let m = &result.metrics;
    // MockExecutor returns cost_usd=0.01, input_tokens=5000, output_tokens=2000,
    // cache_read_tokens=1000, cache_creation_tokens=500 per step
    assert!((m.cost_usd - 0.01).abs() < f64::EPSILON);
    assert_eq!(m.input_tokens, 5000);
    assert_eq!(m.output_tokens, 2000);
    assert_eq!(m.cache_read_tokens, 1000);
    assert_eq!(m.cache_creation_tokens, 500);
    assert!(m.duration.as_nanos() > 0, "duration should be non-zero");
}

#[tokio::test]
async fn metrics_accumulate_across_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "First"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Second"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[1], None, false, "Third"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(2),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();

    let m = &result.metrics;
    // 3 tasks × 0.01 cost each
    assert!((m.cost_usd - 0.03).abs() < f64::EPSILON);
    assert_eq!(m.input_tokens, 15000);
    assert_eq!(m.output_tokens, 6000);
    assert_eq!(m.cache_read_tokens, 3000);
    assert_eq!(m.cache_creation_tokens, 1500);
}

#[tokio::test]
async fn metrics_include_failed_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Will succeed"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::failing(&[1]);
    let config = default_config();
    let result = run_session_with(&repo, TaskId(1), &config, false, Arc::new(executor))
        .await
        .unwrap();

    let m = &result.metrics;
    // Task 0 succeeds (1 step), task 1 fails (max_retries=3 steps)
    let expected_steps = 1 + config.max_retries;
    let expected_cost = f64::from(expected_steps) * 0.01;
    assert!(
        (m.cost_usd - expected_cost).abs() < f64::EPSILON,
        "cost: {} expected: {}",
        m.cost_usd,
        expected_cost
    );
    assert_eq!(m.input_tokens, u64::from(expected_steps) * 5000);
    assert_eq!(m.output_tokens, u64::from(expected_steps) * 2000);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn session_log_contains_metric_events() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    let sessions_dir = repo.join(".rloop").join("sessions");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "First"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Second"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(
        &repo,
        TaskId(1),
        &default_config(),
        false,
        Arc::new(executor),
    )
    .await
    .unwrap();

    // Find the session log file
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1);
    let log_path = entries[0].path();

    let events = load_events(&log_path).unwrap();

    // Should contain StepSummary events (one per task since each succeeds on first try)
    let step_summaries: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::StepSummary { .. }))
        .collect();
    assert_eq!(step_summaries.len(), 2, "one step summary per task");

    // Should contain TaskSummary events
    let task_summaries: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::TaskSummary { .. }))
        .collect();
    assert_eq!(task_summaries.len(), 2, "one task summary per task");

    // Should contain exactly one SessionSummary event
    let session_summaries: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::SessionSummary { .. }))
        .collect();
    assert_eq!(session_summaries.len(), 1, "one session summary");

    // Should contain SessionBanner event
    let banners: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::SessionBanner { .. }))
        .collect();
    assert_eq!(banners.len(), 1, "one session banner");

    // Verify banner fields
    if let Event::SessionBanner {
        session_id,
        datetime,
        target,
        parent_branch,
        session_branch,
    } = &banners[0]
    {
        assert_eq!(session_id, &result.session_id);
        assert!(!datetime.is_empty(), "datetime should be non-empty");
        assert_eq!(*target, Some(TaskId(1)));
        assert_eq!(parent_branch, "main");
        assert!(session_branch.contains("rloop-"));
    } else {
        panic!("expected SessionBanner event");
    }

    // Should contain StepStarted events (one per step)
    let step_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::StepStarted { .. }))
        .collect();
    assert_eq!(step_starts.len(), 2, "one step start per task");

    // Verify step start fields
    if let Event::StepStarted {
        task_id,
        attempt,
        model_id,
        cc_session_id,
    } = &step_starts[0]
    {
        assert_eq!(*task_id, TaskId(0));
        assert_eq!(*attempt, 1);
        assert!(!model_id.is_empty());
        assert!(!cc_session_id.is_empty());
    } else {
        panic!("expected StepStarted event");
    }

    // Verify the session summary has correct aggregated values
    if let Event::SessionSummary {
        session_id,
        cost_usd,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        ..
    } = &session_summaries[0]
    {
        assert_eq!(session_id, &result.session_id);
        assert!((*cost_usd - 0.02).abs() < f64::EPSILON);
        assert_eq!(*input_tokens, 10000);
        assert_eq!(*output_tokens, 4000);
        assert_eq!(*cache_read_tokens, 2000);
        assert_eq!(*cache_creation_tokens, 1000);
    } else {
        panic!("expected SessionSummary event");
    }
}

// ===========================================================================
// Parallel execution tests
// ===========================================================================

fn parallel_config(max_parallel: usize) -> Config {
    Config {
        model: Model::Haiku,
        max_turns: 50,
        max_retries: 3,
        timeout_seconds: 300,
        max_parallel,
        ..Config::default()
    }
}

/// Executor that tracks concurrency: records max simultaneous in-flight tasks.
struct ConcurrencyTrackingExecutor {
    fail_tasks: HashSet<TaskId>,
    current: AtomicUsize,
    peak: AtomicUsize,
    /// Records each executed `TaskId`
    execution_log: Mutex<Vec<TaskId>>,
}

impl ConcurrencyTrackingExecutor {
    fn new(fail_ids: &[u32]) -> Self {
        Self {
            fail_tasks: fail_ids.iter().map(|&id| TaskId(id)).collect(),
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            execution_log: Mutex::new(Vec::new()),
        }
    }

    fn peak_concurrency(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn executed_tasks(&self) -> Vec<TaskId> {
        self.execution_log.lock().unwrap().clone()
    }
}

impl StepExecutor for ConcurrencyTrackingExecutor {
    async fn execute(
        &self,
        task: &Task,
        _worktree_path: &std::path::Path,
        _config: &Config,
        _prior_learnings: &[String],
        _attempt: u32,
    ) -> anyhow::Result<StepResult> {
        let prev = self.current.fetch_add(1, Ordering::SeqCst);
        let concurrent = prev + 1;
        // Update peak
        self.peak.fetch_max(concurrent, Ordering::SeqCst);

        self.execution_log.lock().unwrap().push(task.id);

        // Small delay to allow other tasks to start if they can
        tokio::time::sleep(Duration::from_millis(50)).await;

        self.current.fetch_sub(1, Ordering::SeqCst);

        let outcome = if self.fail_tasks.contains(&task.id) {
            StepOutcome::Failed {
                reason: "mock failure".into(),
                learnings: "mock learning".into(),
            }
        } else {
            StepOutcome::Success {
                summary: "mock success".into(),
            }
        };

        Ok(StepResult {
            task_id: task.id,
            outcome,
            model: Model::Haiku,
            cost_usd: 0.01,
            duration: Duration::from_secs(1),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 100,
            cache_creation_tokens: 50,
            model_id: Some("mock".into()),
            cc_session_id: Some("mock".into()),
            events: vec![],
        })
    }
}

/// Executor that writes a unique file per task in the worktree.
/// Used to test merge conflicts when two tasks modify the same file.
struct FileWritingMockExecutor {
    fail_tasks: HashSet<TaskId>,
    /// Maps task ID to (filename, content) pairs to write in the worktree.
    /// If two tasks write the same filename with different content, a merge
    /// conflict will occur during rebase.
    file_writes: std::collections::HashMap<TaskId, Vec<(String, String)>>,
}

impl FileWritingMockExecutor {
    fn new() -> Self {
        Self {
            fail_tasks: HashSet::new(),
            file_writes: std::collections::HashMap::new(),
        }
    }

    fn add_write(&mut self, task_id: u32, filename: &str, content: &str) {
        self.file_writes
            .entry(TaskId(task_id))
            .or_default()
            .push((filename.to_string(), content.to_string()));
    }
}

impl StepExecutor for FileWritingMockExecutor {
    async fn execute(
        &self,
        task: &Task,
        worktree_path: &std::path::Path,
        _config: &Config,
        _prior_learnings: &[String],
        _attempt: u32,
    ) -> anyhow::Result<StepResult> {
        // Write configured files to the worktree
        if let Some(writes) = self.file_writes.get(&task.id) {
            for (filename, content) in writes {
                let path = worktree_path.join(filename);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, content)?;
            }
        }

        // Yield to allow concurrent tasks to also start and create worktrees
        // from the same session branch base (before any merge happens).
        tokio::time::sleep(Duration::from_millis(50)).await;

        let outcome = if self.fail_tasks.contains(&task.id) {
            StepOutcome::Failed {
                reason: "mock failure".into(),
                learnings: "mock learning".into(),
            }
        } else {
            StepOutcome::Success {
                summary: "mock success".into(),
            }
        };

        Ok(StepResult {
            task_id: task.id,
            outcome,
            model: Model::Haiku,
            cost_usd: 0.01,
            duration: Duration::from_secs(1),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 100,
            cache_creation_tokens: 50,
            model_id: Some("mock".into()),
            cc_session_id: Some("mock".into()),
            events: vec![],
        })
    }
}

// --- Diamond DAG: 0 → {1, 2} → 3; verify 1 and 2 can run in parallel ---

#[tokio::test]
async fn parallel_diamond_dag() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Diamond: 0 is root, 1 and 2 depend on 0, 3 depends on 1 and 2
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Left branch"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Right branch"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[1, 2], None, false, "Merge point"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(3), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert_eq!(result.completed_tasks.len(), 4);
    assert!(result.failed_tasks.is_empty());
    // With max_parallel=2, tasks 1 and 2 should run concurrently
    assert!(
        executor.peak_concurrency() >= 2,
        "expected peak concurrency >= 2, got {}",
        executor.peak_concurrency()
    );
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

// --- Wide DAG: 5 independent tasks, max_parallel=2, verify throttling ---

#[tokio::test]
async fn parallel_wide_dag_throttled() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // 5 independent tasks + a root that depends on all of them
    for i in 0..5 {
        helpers::create_task_file(
            &tasks_dir,
            i,
            &helpers::minimal_task(i, &format!("Task {i}")),
        );
    }
    helpers::create_task_file(
        &tasks_dir,
        5,
        &helpers::full_task(5, "Sonnet", &[0, 1, 2, 3, 4], None, false, "Collector"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(5), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert_eq!(result.completed_tasks.len(), 6);
    assert!(result.failed_tasks.is_empty());
    // Peak concurrency should not exceed max_parallel=2
    assert!(
        executor.peak_concurrency() <= 2,
        "expected peak concurrency <= 2, got {}",
        executor.peak_concurrency()
    );
}

// --- Failure mid-flight: task fails while sibling runs ---

#[tokio::test]
async fn parallel_failure_mid_flight() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Root: 0; siblings 1 (fails) and 2 (succeeds); 3 depends on both
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Will succeed"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[1, 2], None, false, "Blocked"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[1]));
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(3), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert!(result.completed_tasks.contains(&TaskId(0)));
    // Task 1 failed, task 2 should still succeed
    assert!(result.failed_tasks.contains(&TaskId(1)));
    assert!(result.completed_tasks.contains(&TaskId(2)));
    // Task 3 is blocked by failed task 1
    assert!(result.skipped_tasks.contains(&TaskId(3)));
}

// --- max_parallel=1: existing tests all pass through new code path ---

#[tokio::test]
async fn max_parallel_one_linear_chain() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "First"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Second"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[1], None, false, "Third"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(1);
    let result = run_session_with(&repo, TaskId(2), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert_eq!(result.completed_tasks.len(), 3);
    // With max_parallel=1, peak concurrency should be exactly 1
    assert_eq!(
        executor.peak_concurrency(),
        1,
        "max_parallel=1 should run sequentially"
    );
    // Tasks should execute in order
    let executed = executor.executed_tasks();
    assert_eq!(executed, vec![TaskId(0), TaskId(1), TaskId(2)]);
}

#[tokio::test]
async fn max_parallel_one_failure_blocks_dependent() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Base"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[1], None, false, "Blocked"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[1]));
    let config = parallel_config(1);
    let result = run_session_with(&repo, TaskId(2), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert!(result.completed_tasks.contains(&TaskId(0)));
    assert!(result.failed_tasks.contains(&TaskId(1)));
    assert!(result.skipped_tasks.contains(&TaskId(2)));
}

// --- Merge conflict: two tasks write conflicting files ---
//
// For a merge conflict to occur, both tasks must branch off the same session
// branch commit (before either has merged). This requires max_parallel >= 2
// so both worktrees are created from the same base.
//
// With the merge-fix agent (Phase 3), simple conflicts are resolved automatically.
// The rebase fails, then the merge-fix agent is invoked and resolves the conflict.

#[tokio::test]
async fn parallel_merge_conflict_preserves_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Two independent tasks that both write to the same file.
    // With max_parallel=2, both branch off the same session commit.
    // When the second to merge tries to rebase, it will conflict.
    // The merge-fix agent should resolve the simple conflict.
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Write version A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Write version B"));
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0, 1], None, false, "Depends on both"),
    );
    helpers::commit_all(&repo, "add tasks");

    let mut executor = FileWritingMockExecutor::new();
    // Both tasks write different content to the same file
    executor.add_write(0, "shared.txt", "version A\n");
    executor.add_write(1, "shared.txt", "version B\n");

    // max_parallel=2: both tasks run concurrently from the same base.
    // The second to merge will conflict during rebase, but the merge-fix
    // agent should resolve it.
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(2), &config, false, Arc::new(executor))
        .await
        .unwrap();

    // The merge-fix agent should resolve the simple conflict, so all 3 tasks complete.
    assert_eq!(
        result.completed_tasks.len(),
        3,
        "merge-fix agent should resolve simple conflict; completed: {:?}, failed: {:?}",
        result.completed_tasks,
        result.failed_tasks
    );
    assert!(
        result.failed_tasks.is_empty(),
        "no tasks should fail with merge-fix agent; failed: {:?}",
        result.failed_tasks
    );

    // Main working tree should still be on main
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

#[tokio::test]
async fn parallel_merge_conflict_logs_merge_fix_events() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    let sessions_dir = repo.join(".rloop").join("sessions");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Write A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Write B"));
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0, 1], None, false, "Collector"),
    );
    helpers::commit_all(&repo, "add tasks");

    let mut executor = FileWritingMockExecutor::new();
    executor.add_write(0, "conflict.txt", "content A\n");
    executor.add_write(1, "conflict.txt", "content B\n");

    // max_parallel=2: concurrent execution causes divergent branches
    let config = parallel_config(2);
    let _result = run_session_with(&repo, TaskId(2), &config, false, Arc::new(executor))
        .await
        .unwrap();

    // Check session log for merge-fix events
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(!entries.is_empty());
    let log_path = entries[0].path();
    let events = load_events(&log_path).unwrap();

    // Should have MergeFixStarted event (rebase failed, merge-fix invoked)
    let merge_fix_started: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::MergeFixStarted { .. }))
        .collect();
    assert!(
        !merge_fix_started.is_empty(),
        "should have at least one MergeFixStarted event"
    );

    // Should have MergeFixSucceeded event (agent resolved the conflict)
    let merge_fix_succeeded: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::MergeFixSucceeded { .. }))
        .collect();
    assert!(
        !merge_fix_succeeded.is_empty(),
        "should have at least one MergeFixSucceeded event"
    );

    // Verify MergeFixSucceeded has cost_usd field
    if let Event::MergeFixSucceeded { task_id, attempt, cost_usd } = &merge_fix_succeeded[0] {
        assert!(
            *task_id == TaskId(0) || *task_id == TaskId(1),
            "merge-fix task should be 0 or 1"
        );
        assert!(*attempt >= 1, "attempt should be at least 1");
        assert!(*cost_usd >= 0.0, "cost_usd should be non-negative");
    } else {
        panic!("expected MergeFixSucceeded event");
    }
}

// --- Non-conflicting parallel writes merge correctly ---

#[tokio::test]
async fn parallel_non_conflicting_writes_merge() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Two independent tasks writing different files
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Write file A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Write file B"));
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0, 1], None, false, "Depends on both"),
    );
    helpers::commit_all(&repo, "add tasks");

    let mut executor = FileWritingMockExecutor::new();
    executor.add_write(0, "file_a.txt", "content A\n");
    executor.add_write(1, "file_b.txt", "content B\n");

    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(2), &config, false, Arc::new(executor))
        .await
        .unwrap();

    // All tasks should complete
    assert_eq!(result.completed_tasks.len(), 3);
    assert!(result.failed_tasks.is_empty());
}

// ===========================================================================
// Phase 4: Comprehensive parallel execution tests
// ===========================================================================

// --- Diamond DAG with max_parallel=2: verify true concurrency ---

#[tokio::test]
async fn parallel_diamond_concurrent_execution() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // 0 → {1, 2} → 3
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Left"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Right"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[1, 2], None, false, "Merge"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(3), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    // All 4 tasks complete
    assert_eq!(result.completed_tasks.len(), 4);
    assert!(result.failed_tasks.is_empty());
    assert!(result.skipped_tasks.is_empty());

    // Tasks 1 and 2 ran concurrently (peak >= 2)
    assert!(
        executor.peak_concurrency() >= 2,
        "diamond branches should run concurrently, got peak={}",
        executor.peak_concurrency()
    );

    // Execution order: 0 must run first, 3 must run last
    let log = executor.executed_tasks();
    let pos_0 = log.iter().position(|t| *t == TaskId(0)).unwrap();
    let pos_3 = log.iter().position(|t| *t == TaskId(3)).unwrap();
    assert_eq!(pos_0, 0, "root task should run first");
    assert_eq!(pos_3, log.len() - 1, "merge task should run last");
}

// --- Wide independent DAG: 5 tasks, max_parallel=3, verify throttling ---

#[tokio::test]
async fn parallel_wide_five_tasks_throttled_to_three() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // 5 independent tasks + a collector
    for i in 0..5 {
        helpers::create_task_file(
            &tasks_dir,
            i,
            &helpers::minimal_task(i, &format!("Independent {i}")),
        );
    }
    helpers::create_task_file(
        &tasks_dir,
        5,
        &helpers::full_task(5, "Sonnet", &[0, 1, 2, 3, 4], None, false, "Collector"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(3);
    let result = run_session_with(&repo, TaskId(5), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert_eq!(result.completed_tasks.len(), 6);
    assert!(result.failed_tasks.is_empty());
    // Peak concurrency should not exceed max_parallel=3
    assert!(
        executor.peak_concurrency() <= 3,
        "peak concurrency should be <= 3, got {}",
        executor.peak_concurrency()
    );
    // Should actually use concurrency (peak >= 2 with 5 independent tasks)
    assert!(
        executor.peak_concurrency() >= 2,
        "should use concurrency with 5 independent tasks, got peak={}",
        executor.peak_concurrency()
    );
}

// --- Merge conflict with merge-fix: verify agent is invoked ---

#[tokio::test]
async fn parallel_merge_conflict_invokes_merge_fix() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    let sessions_dir = repo.join(".rloop").join("sessions");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Two independent tasks writing conflicting files
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Write A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Write B"));
    helpers::commit_all(&repo, "add tasks");

    let mut executor = FileWritingMockExecutor::new();
    executor.add_write(0, "shared.txt", "content from task 0\n");
    executor.add_write(1, "shared.txt", "content from task 1\n");

    let config = parallel_config(2);
    let _result = run_all_session_with(&repo, &config, false, Arc::new(executor))
        .await
        .unwrap();

    // Verify merge-fix events in session log
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(!entries.is_empty());
    let log_path = entries[0].path();
    let events = load_events(&log_path).unwrap();

    let merge_fix_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::MergeFixStarted { .. } | Event::MergeFixSucceeded { .. }
            )
        })
        .collect();
    assert!(
        !merge_fix_events.is_empty(),
        "merge-fix agent should be invoked for conflicting writes"
    );
}

// --- Failure propagation: task fails mid-flight, sibling completes ---

#[tokio::test]
async fn parallel_failure_sibling_completes_dependent_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Use run_all to include all tasks in the execution plan.
    // Root: 0; siblings: 1 (fails), 2 (succeeds), 3 (independent of 0)
    // Dependent: 4 depends on 1 and 2
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Will succeed"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::minimal_task(3, "Independent of DAG"),
    );
    helpers::create_task_file(
        &tasks_dir,
        4,
        &helpers::full_task(4, "Sonnet", &[1, 2], None, false, "Blocked by 1"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[1]));
    let config = parallel_config(3);
    let result = run_all_session_with(&repo, &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    // Root succeeded
    assert!(result.completed_tasks.contains(&TaskId(0)));
    // Task 1 failed
    assert!(result.failed_tasks.contains(&TaskId(1)));
    // Task 2 should still succeed (independent sibling)
    assert!(result.completed_tasks.contains(&TaskId(2)));
    // Task 3 is independent and should succeed
    assert!(result.completed_tasks.contains(&TaskId(3)));
    // Task 4 is blocked by task 1
    assert!(result.skipped_tasks.contains(&TaskId(4)));
}

// --- All-tasks parallel: run_all_session_with max_parallel > 1 ---

#[tokio::test]
async fn parallel_run_all_with_concurrency() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // 3 independent tasks
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Task B"));
    helpers::create_task_file(&tasks_dir, 2, &helpers::minimal_task(2, "Task C"));
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let result = run_all_session_with(&repo, &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    assert_eq!(result.completed_tasks.len(), 3);
    assert!(result.failed_tasks.is_empty());
    assert!(result.branch.contains("rloop-all-"));
    // Should actually use concurrency with 3 independent tasks and max_parallel=2
    assert!(
        executor.peak_concurrency() >= 2,
        "should use concurrency, got peak={}",
        executor.peak_concurrency()
    );
    assert_eq!(rloop::git::current_branch(&repo).unwrap(), "main");
}

// --- Session metrics aggregate correctly across parallel tasks ---

#[tokio::test]
async fn parallel_metrics_aggregate_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Diamond: 0 → {1, 2} → 3
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Left"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Right"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[1, 2], None, false, "Merge"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(3), &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    let m = &result.metrics;
    // 4 tasks × 0.01 cost each (ConcurrencyTrackingExecutor returns 0.01 per step)
    assert!(
        (m.cost_usd - 0.04).abs() < f64::EPSILON,
        "cost: {} expected: 0.04",
        m.cost_usd
    );
    assert_eq!(m.input_tokens, 4000); // 4 × 1000
    assert_eq!(m.output_tokens, 2000); // 4 × 500
    assert_eq!(m.cache_read_tokens, 400); // 4 × 100
    assert_eq!(m.cache_creation_tokens, 200); // 4 × 50
    assert!(m.duration.as_nanos() > 0, "duration should be non-zero");
}

// --- Session log events for parallel execution ---

#[tokio::test]
async fn parallel_session_log_contains_all_events() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    let sessions_dir = repo.join(".rloop").join("sessions");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // 3 independent tasks
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "B"));
    helpers::create_task_file(&tasks_dir, 2, &helpers::minimal_task(2, "C"));
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let _result = run_all_session_with(&repo, &config, false, Arc::clone(&executor))
        .await
        .unwrap();

    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1);
    let events = load_events(entries[0].path()).unwrap();

    // Should have TaskStarted for each task
    let task_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::TaskStarted { .. }))
        .collect();
    assert_eq!(task_starts.len(), 3, "one TaskStarted per task");

    // Should have TaskCompleted events for each task (one per successful task)
    let task_completes: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::TaskCompleted { .. }))
        .collect();
    assert_eq!(
        task_completes.len(),
        3,
        "one TaskCompleted per successful task"
    );

    // Should have StepSummary for each task
    let step_summaries: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::StepSummary { .. }))
        .collect();
    assert_eq!(step_summaries.len(), 3, "one StepSummary per task");

    // Should have TaskSummary for each task
    let task_summaries: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::TaskSummary { .. }))
        .collect();
    assert_eq!(task_summaries.len(), 3, "one TaskSummary per task");

    // Should have exactly one SessionSummary
    let session_summaries: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::SessionSummary { .. }))
        .collect();
    assert_eq!(session_summaries.len(), 1, "one SessionSummary");
}

// --- Ctrl+C cleanup: verify worktrees dir is clean after cleanup ---

#[tokio::test]
async fn cleanup_worktrees_removes_all() {
    // This test creates worktrees manually and verifies the cleanup function
    // from main.rs works correctly for multiple simultaneous worktrees.
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let worktrees_dir = repo.join(".rloop").join("worktrees");
    std::fs::create_dir_all(&worktrees_dir).unwrap();

    // Create session branch
    rloop::git::git(&repo, &["branch", "rloop-test"]).unwrap();

    // Create multiple worktrees (simulating parallel execution)
    let wt0 = rloop::git::create_worktree(&repo, TaskId(0), "rloop-test").unwrap();
    let wt1 = rloop::git::create_worktree(&repo, TaskId(1), "rloop-test").unwrap();
    let wt2 = rloop::git::create_worktree(&repo, TaskId(2), "rloop-test").unwrap();

    // Verify worktrees exist
    assert!(wt0.path.exists());
    assert!(wt1.path.exists());
    assert!(wt2.path.exists());

    // Clean up all worktrees (simulating Ctrl+C handler)
    for wt in &[&wt0, &wt1, &wt2] {
        rloop::git::cleanup_worktree(&repo, wt).unwrap();
    }

    // Verify all worktrees are removed
    assert!(!wt0.path.exists());
    assert!(!wt1.path.exists());
    assert!(!wt2.path.exists());

    // Verify branches are deleted
    let branches = rloop::git::git(&repo, &["branch"]).unwrap();
    assert!(!branches.contains("rloop-wt-00"));
    assert!(!branches.contains("rloop-wt-01"));
    assert!(!branches.contains("rloop-wt-02"));
}

// --- Ctrl+C: verify scanning .rloop/worktrees/ finds all entries ---

#[tokio::test]
async fn cleanup_scan_finds_parallel_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let worktrees_dir = repo.join(".rloop").join("worktrees");
    std::fs::create_dir_all(&worktrees_dir).unwrap();

    rloop::git::git(&repo, &["branch", "rloop-test"]).unwrap();

    // Create 3 worktrees
    let _wt0 = rloop::git::create_worktree(&repo, TaskId(0), "rloop-test").unwrap();
    let _wt1 = rloop::git::create_worktree(&repo, TaskId(1), "rloop-test").unwrap();
    let _wt2 = rloop::git::create_worktree(&repo, TaskId(2), "rloop-test").unwrap();

    // Scan worktrees directory (same logic as cleanup_worktrees in main.rs)
    let entries: Vec<_> = std::fs::read_dir(&worktrees_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(entries.len(), 3, "should find 3 worktree directories");

    // Parse task IDs from directory names
    let mut ids: Vec<u32> = entries
        .iter()
        .filter_map(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.parse::<u32>().ok())
        })
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 2]);

    // Clean up for proper test teardown
    for entry in &entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_str().unwrap();
        let id: u32 = name.parse().unwrap();
        let wt = rloop::git::Worktree {
            path: path.clone(),
            branch: format!("rloop-wt-{name}"),
            task_id: TaskId(id),
        };
        rloop::git::cleanup_worktree(&repo, &wt).unwrap();
    }
}

// --- Cleanup handles merge-fix worktrees ---

#[tokio::test]
async fn cleanup_handles_mergefix_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let worktrees_dir = repo.join(".rloop").join("worktrees");
    std::fs::create_dir_all(&worktrees_dir).unwrap();

    // Create session branch
    rloop::git::git(&repo, &["branch", "rloop-test"]).unwrap();

    // Create a regular worktree
    let wt0 = rloop::git::create_worktree(&repo, TaskId(0), "rloop-test").unwrap();

    // Create a merge-fix worktree manually (simulating what merge.rs does)
    let mergefix_dir = worktrees_dir.join("01-mergefix");
    let mergefix_branch = "rloop-mergefix-01";
    rloop::git::git(
        &repo,
        &[
            "worktree",
            "add",
            mergefix_dir.to_str().unwrap(),
            "-b",
            mergefix_branch,
            "rloop-test",
        ],
    )
    .unwrap();

    // Verify both exist
    assert!(wt0.path.exists());
    assert!(mergefix_dir.exists());

    // Verify branches exist
    let branches = rloop::git::git(&repo, &["branch"]).unwrap();
    assert!(branches.contains("rloop-wt-00"));
    assert!(branches.contains(mergefix_branch));

    // Run cleanup logic (same as main.rs cleanup_worktrees)
    let entries: Vec<_> = std::fs::read_dir(&worktrees_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();

    for entry in entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_str().unwrap();

        let (task_id, branch) = if let Some(id_str) = name.strip_suffix("-mergefix") {
            let id: u32 = id_str.parse().unwrap();
            (TaskId(id), format!("rloop-mergefix-{id_str}"))
        } else {
            let id: u32 = name.parse().unwrap();
            (TaskId(id), format!("rloop-wt-{name}"))
        };

        let wt = rloop::git::Worktree {
            path: path.clone(),
            branch,
            task_id,
        };
        rloop::git::cleanup_worktree(&repo, &wt).unwrap();
    }

    // Verify both worktrees are removed
    assert!(!wt0.path.exists());
    assert!(!mergefix_dir.exists());

    // Verify both branches are deleted
    let branches = rloop::git::git(&repo, &["branch"]).unwrap();
    assert!(!branches.contains("rloop-wt-00"));
    assert!(!branches.contains("rloop-mergefix-01"));
}

// --- Non-conflicting parallel writes with different files merge cleanly ---

#[tokio::test]
async fn parallel_three_tasks_write_different_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // 3 independent tasks writing different files
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Write A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Write B"));
    helpers::create_task_file(&tasks_dir, 2, &helpers::minimal_task(2, "Write C"));
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[0, 1, 2], None, false, "Collector"),
    );
    helpers::commit_all(&repo, "add tasks");

    let mut executor = FileWritingMockExecutor::new();
    executor.add_write(0, "file_a.txt", "content A\n");
    executor.add_write(1, "file_b.txt", "content B\n");
    executor.add_write(2, "file_c.txt", "content C\n");

    let config = parallel_config(3);
    let result = run_session_with(&repo, TaskId(3), &config, false, Arc::new(executor))
        .await
        .unwrap();

    assert_eq!(result.completed_tasks.len(), 4);
    assert!(result.failed_tasks.is_empty());
}

// --- Auto-merge works with parallel execution ---

#[tokio::test]
async fn parallel_auto_merge() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "A"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "B"));
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0, 1], None, false, "Merge"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[]));
    let config = parallel_config(2);
    let result = run_session_with(&repo, TaskId(2), &config, true, Arc::clone(&executor))
        .await
        .unwrap();

    assert!(result.auto_merged);
    assert_eq!(result.completed_tasks.len(), 3);
}

// --- Failure prevents auto-merge in parallel ---

#[tokio::test]
async fn parallel_failure_prevents_auto_merge() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Fails"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Succeeds"),
    );
    helpers::commit_all(&repo, "add tasks");

    let executor = Arc::new(ConcurrencyTrackingExecutor::new(&[1]));
    let config = parallel_config(2);
    let result = run_all_session_with(&repo, &config, true, Arc::clone(&executor))
        .await
        .unwrap();

    // Auto-merge should not happen because task 1 failed
    assert!(!result.auto_merged);
    assert!(result.failed_tasks.contains(&TaskId(1)));
    assert!(result.completed_tasks.contains(&TaskId(2)));
}

/// Executor that returns an error (not a failed task, but an actual system error)
/// on specific task IDs to simulate internal failures.
struct ErrorExecutor {
    error_tasks: HashSet<TaskId>,
}

impl ErrorExecutor {
    fn erroring(error_ids: &[u32]) -> Self {
        Self {
            error_tasks: error_ids.iter().map(|&id| TaskId(id)).collect(),
        }
    }
}

impl StepExecutor for ErrorExecutor {
    async fn execute(
        &self,
        task: &Task,
        _worktree_path: &std::path::Path,
        _config: &Config,
        _prior_learnings: &[String],
        _attempt: u32,
    ) -> anyhow::Result<StepResult> {
        if self.error_tasks.contains(&task.id) {
            anyhow::bail!("simulated system error for task {}", task.id.0);
        }
        Ok(StepResult {
            task_id: task.id,
            outcome: StepOutcome::Success {
                summary: "mock success".into(),
            },
            model: Model::Haiku,
            cost_usd: 0.01,
            duration: Duration::from_secs(10),
            input_tokens: 5000,
            output_tokens: 2000,
            cache_read_tokens: 1000,
            cache_creation_tokens: 500,
            model_id: Some("claude-haiku-4-5-20251001".into()),
            cc_session_id: Some("mock-session-id".into()),
            events: vec![],
        })
    }
}

#[tokio::test]
async fn worktrees_cleaned_up_on_executor_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Create three independent tasks
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "First"));
    helpers::create_task_file(&tasks_dir, 1, &helpers::minimal_task(1, "Will error"));
    helpers::create_task_file(&tasks_dir, 2, &helpers::minimal_task(2, "Third"));
    helpers::commit_all(&repo, "add tasks");

    // Task 1 will return a system error (not a failed task outcome)
    let executor = ErrorExecutor::erroring(&[1]);
    let config = parallel_config(3); // Allow parallel execution

    let err = run_all_session_with(&repo, &config, false, Arc::new(executor))
        .await
        .unwrap_err();

    // Verify the error is the one we expect
    let msg = format!("{err:#}");
    assert!(
        msg.contains("simulated system error"),
        "expected simulated error, got: {msg}"
    );

    // Verify no worktrees remain
    let worktrees_dir = repo.join(".rloop").join("worktrees");
    if worktrees_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&worktrees_dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            entries.is_empty(),
            "expected all worktrees to be cleaned up, found: {:?}",
            entries
                .iter()
                .map(std::fs::DirEntry::file_name)
                .collect::<Vec<_>>()
        );
    }

    // Verify no worktree branches remain
    for task_id in [0, 1, 2] {
        let branch = format!("rloop-wt-{task_id:02}");
        let exists = rloop::git::branch_exists(&repo, &branch).unwrap();
        assert!(
            !exists,
            "worktree branch {branch} should have been cleaned up"
        );
    }
}
