mod helpers;

use std::collections::HashSet;
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
            cc_session_id: None,
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
    let result = run_session_with(&repo, TaskId(0), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(2), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(1), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(2), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(3), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(5), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(0), &default_config(), true, &executor)
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
    let result = run_session_with(&repo, TaskId(0), &default_config(), true, &executor)
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
    let _result = run_session_with(&repo, TaskId(0), &default_config(), false, &executor)
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
    let result = run_all_session_with(&repo, &default_config(), false, &executor)
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
    let result = run_all_session_with(&repo, &default_config(), false, &executor)
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
    let result = run_all_session_with(&repo, &default_config(), false, &executor)
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
    let err = run_session_with(dir.path(), TaskId(0), &default_config(), false, &executor)
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
    let err = run_session_with(&repo, TaskId(0), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(0), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(2), &default_config(), false, &executor)
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
    let result = run_session_with(&repo, TaskId(1), &config, false, &executor)
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
    let result = run_session_with(&repo, TaskId(1), &default_config(), false, &executor)
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
