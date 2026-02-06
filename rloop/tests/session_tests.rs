mod helpers;

use std::collections::HashSet;
use std::time::Duration;

use rloop::config::Config;
use rloop::model::Model;
use rloop::session::{run_all_session_with, run_session_with};
use rloop::step::{StepExecutor, StepOutcome, StepResult};
use rloop::task::{Task, TaskId};

fn default_config() -> Config {
    Config {
        model: Model::Haiku,
        max_turns: 50,
        max_retries: 3,
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
            cost_usd: 0.0,
            duration: Duration::ZERO,
            cc_session_id: None,
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

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(&repo, TaskId(0), &default_config(), false, &executor)
        .await
        .unwrap();
    assert!(result.completed_tasks.contains(&TaskId(0)));
    assert!(result.failed_tasks.is_empty());
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

    let executor = MockExecutor::failing(&[1]);
    let result = run_session_with(&repo, TaskId(2), &default_config(), false, &executor)
        .await
        .unwrap();
    assert!(result.completed_tasks.contains(&TaskId(0)));
    assert!(result.failed_tasks.contains(&TaskId(1)));
    assert!(result.skipped_tasks.contains(&TaskId(2)));
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

    let executor = MockExecutor::all_succeed();
    let result = run_session_with(&repo, TaskId(5), &default_config(), false, &executor)
        .await
        .unwrap();
    assert_eq!(result.branch, "rloop/05");
}

#[tokio::test]
async fn auto_merge_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));

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

    let executor = MockExecutor::all_succeed();
    let result = run_all_session_with(&repo, &default_config(), false, &executor)
        .await
        .unwrap();
    assert_eq!(result.completed_tasks.len(), 3);
    assert!(result.branch.contains("rloop/all-"));
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

    let executor = MockExecutor::all_succeed();
    let result = run_all_session_with(&repo, &default_config(), false, &executor)
        .await
        .unwrap();
    assert!(result.completed_tasks.is_empty());
}
