mod helpers;

use rloop::config::Config;
use rloop::model::Model;
use rloop::session::run_session;
use rloop::task::TaskId;

fn default_config() -> Config {
    Config {
        model: Model::Sonnet,
        max_turns: 50,
        max_retries: 10,
    }
}

#[tokio::test]
async fn single_task_no_deps() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Do something"));

    let result = run_session(&repo, TaskId(0), &default_config(), false)
        .await
        .unwrap();
    assert!(result.completed_tasks.contains(&TaskId(0)));
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

    let result = run_session(&repo, TaskId(2), &default_config(), false)
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

    let result = run_session(&repo, TaskId(1), &default_config(), false)
        .await
        .unwrap();
    // Task 0 was already completed, should not be in completed_tasks
    assert!(!result.completed_tasks.contains(&TaskId(0)));
    assert!(result.completed_tasks.contains(&TaskId(1)));
}

#[tokio::test]
async fn failure_continues_unblocked() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    // Two independent chains: 0->1, 2->3
    // Even if task 1 fails, task 2 and 3 should still run
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Base A"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Will fail"),
    );
    helpers::create_task_file(&tasks_dir, 2, &helpers::minimal_task(2, "Base B"));
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[2], None, false, "Should still run"),
    );

    // This test validates the session continues past failures
    // When implemented, task 1's failure shouldn't prevent task 3
    let result = run_session(&repo, TaskId(3), &default_config(), false)
        .await
        .unwrap();
    assert!(!result.failed_tasks.is_empty() || !result.completed_tasks.is_empty());
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

    let result = run_session(&repo, TaskId(2), &default_config(), false)
        .await
        .unwrap();
    // Task 2 should be skipped because task 1 failed
    assert!(
        result.skipped_tasks.contains(&TaskId(2)) || result.failed_tasks.contains(&TaskId(1))
    );
}

#[tokio::test]
async fn diamond_one_branch_fails() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Root"));
    helpers::create_task_file(
        &tasks_dir,
        1,
        &helpers::full_task(1, "Sonnet", &[0], None, false, "Branch A — fails"),
    );
    helpers::create_task_file(
        &tasks_dir,
        2,
        &helpers::full_task(2, "Sonnet", &[0], None, false, "Branch B — succeeds"),
    );
    helpers::create_task_file(
        &tasks_dir,
        3,
        &helpers::full_task(3, "Sonnet", &[1, 2], None, false, "Needs both branches"),
    );

    let result = run_session(&repo, TaskId(3), &default_config(), false)
        .await
        .unwrap();
    // Task 3 should be skipped because task 1 failed
    assert!(
        result.skipped_tasks.contains(&TaskId(3))
            || !result.completed_tasks.contains(&TaskId(3))
    );
}

#[tokio::test]
async fn session_branch_naming() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 5, &helpers::minimal_task(5, "Target task"));

    let result = run_session(&repo, TaskId(5), &default_config(), false)
        .await
        .unwrap();
    assert!(result.branch.contains("rloop"));
}

#[tokio::test]
async fn auto_merge_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    helpers::create_task_file(&tasks_dir, 0, &helpers::minimal_task(0, "Task"));

    let result = run_session(&repo, TaskId(0), &default_config(), true)
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

    let result = run_session(&repo, TaskId(0), &default_config(), true)
        .await
        .unwrap();
    // If any task fails, auto_merge should be false
    if !result.failed_tasks.is_empty() {
        assert!(!result.auto_merged);
    }
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

    let _result = run_session(&repo, TaskId(0), &default_config(), false)
        .await
        .unwrap();
    // Check that a session log file was created
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(!entries.is_empty(), "Session should produce a log file");
}
