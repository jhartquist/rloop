use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use anyhow::Context;
use serde::Serialize;

use crate::config::Config;
use crate::git::{
    self, Worktree, cleanup_worktree, create_worktree, fast_forward_ref, lock_worktree,
};
use crate::log::{Event, SessionLogger};
use crate::step::{RealStepExecutor, StepExecutor, StepOutcome};
use crate::task::{Task, TaskId, load_all_tasks, resolve_all_dag, resolve_dag};

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
#[derive(Debug)]
pub struct SessionResult {
    pub session_id: SessionId,
    pub branch: String,
    pub completed_tasks: Vec<TaskId>,
    pub failed_tasks: Vec<TaskId>,
    pub skipped_tasks: Vec<TaskId>,
    pub auto_merged: bool,
}

// ---------------------------------------------------------------------------
// Git helpers (session-level branching)
// ---------------------------------------------------------------------------

fn create_session_branch(repo: &Path, name: &str) -> anyhow::Result<()> {
    git::git(repo, &["branch", name])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Logging helpers
// ---------------------------------------------------------------------------

fn emit(logger: &mut SessionLogger, event: &Event) {
    logger.print(event);
    // Best-effort log — don't abort session on log write failure
    let _ = logger.log(event);
}

// ---------------------------------------------------------------------------
// Task execution (single task within a session)
// ---------------------------------------------------------------------------

/// Outcome of running a single task within a session (including retries).
enum TaskOutcome {
    Success,
    Failed,
}

/// Execute a single task: create worktree, run agent with retries, merge or cleanup.
async fn execute_task<E: StepExecutor>(
    repo: &Path,
    task: &Task,
    session_branch: &str,
    config: &Config,
    logger: &mut SessionLogger,
    executor: &E,
) -> anyhow::Result<TaskOutcome> {
    let task_id = task.id;

    emit(logger, &Event::TaskStarted { task_id });

    // Create worktree off session branch
    let worktree = create_worktree(repo, task_id, session_branch)
        .with_context(|| format!("failed to create worktree for task {task_id}"))?;

    emit(
        logger,
        &Event::WorktreeCreated {
            task_id,
            path: worktree.path.clone(),
        },
    );

    // Lock worktree (non-fatal if it fails)
    if let Err(e) = lock_worktree(&worktree) {
        eprintln!("warning: failed to lock worktree for task {task_id}: {e}");
    }

    // Execute with retries
    let succeeded = run_with_retries(task, &worktree, config, logger, executor).await?;

    if succeeded {
        finish_successful_task(repo, &worktree, session_branch, task_id, logger)?;
        Ok(TaskOutcome::Success)
    } else {
        finish_failed_task(repo, &worktree, task_id, logger)?;
        Ok(TaskOutcome::Failed)
    }
}

/// Retry loop: run `execute_step` up to `max_retries` times, accumulating learnings.
async fn run_with_retries<E: StepExecutor>(
    task: &Task,
    worktree: &Worktree,
    config: &Config,
    logger: &mut SessionLogger,
    executor: &E,
) -> anyhow::Result<bool> {
    let task_id = task.id;
    let mut attempts: u32 = 0;
    let mut prior_learnings: Vec<String> = Vec::new();

    loop {
        attempts += 1;
        emit(logger, &Event::PromptSent { task_id });

        let step_result = executor
            .execute(task, &worktree.path, config, &prior_learnings)
            .await?;

        match &step_result.outcome {
            StepOutcome::Success { summary } => {
                emit(
                    logger,
                    &Event::TaskCompleted {
                        task_id,
                        summary: summary.clone(),
                    },
                );
                return Ok(true);
            }
            StepOutcome::Failed { reason, learnings } => {
                prior_learnings.push(learnings.clone());

                if attempts >= config.max_retries {
                    emit(
                        logger,
                        &Event::TaskFailed {
                            task_id,
                            reason: format!(
                                "{reason} (after {attempts} attempt{})",
                                if attempts == 1 { "" } else { "s" }
                            ),
                        },
                    );
                    return Ok(false);
                }
            }
        }
    }
}

/// Fast-forward the session branch to the task's tip and clean up.
fn finish_successful_task(
    repo: &Path,
    worktree: &Worktree,
    session_branch: &str,
    task_id: TaskId,
    logger: &mut SessionLogger,
) -> anyhow::Result<()> {
    fast_forward_ref(repo, session_branch, &worktree.branch)
        .with_context(|| format!("failed to fast-forward session branch for task {task_id}"))?;

    emit(
        logger,
        &Event::WorktreeMerged {
            task_id,
            into_branch: session_branch.to_string(),
        },
    );

    cleanup_worktree(repo, worktree)
        .with_context(|| format!("failed to cleanup worktree for task {task_id}"))?;

    emit(logger, &Event::WorktreeCleanedUp { task_id });
    Ok(())
}

/// Clean up a failed task's worktree.
fn finish_failed_task(
    repo: &Path,
    worktree: &Worktree,
    task_id: TaskId,
    logger: &mut SessionLogger,
) -> anyhow::Result<()> {
    cleanup_worktree(repo, worktree)
        .with_context(|| format!("failed to cleanup worktree for failed task {task_id}"))?;

    emit(logger, &Event::WorktreeCleanedUp { task_id });
    Ok(())
}

// ---------------------------------------------------------------------------
// Session orchestration
// ---------------------------------------------------------------------------

/// Run a session with the default (real) executor.
///
/// # Errors
/// Returns an error if session initialization, task loading, DAG resolution, or execution fails.
pub async fn run_session(
    repo: impl AsRef<Path>,
    target_task: TaskId,
    config: &Config,
    auto_merge: bool,
) -> anyhow::Result<SessionResult> {
    run_session_with(repo, target_task, config, auto_merge, &RealStepExecutor).await
}

/// Run a session with a custom executor (for testing).
///
/// # Errors
/// Returns an error if session initialization, task loading, DAG resolution, or execution fails.
pub async fn run_session_with<E: StepExecutor>(
    repo: impl AsRef<Path>,
    target_task: TaskId,
    config: &Config,
    auto_merge: bool,
    executor: &E,
) -> anyhow::Result<SessionResult> {
    let repo = repo.as_ref();

    git::assert_has_commits(repo)?;
    git::assert_clean_worktree(repo)?;

    let session_id = SessionId(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let session_dir = repo.join(".rloop").join("sessions");
    let mut logger = SessionLogger::new(&session_dir, session_id.clone())?;

    emit(
        &mut logger,
        &Event::SessionStarted {
            session_id: session_id.clone(),
            target: target_task,
        },
    );

    let parent_branch = git::current_branch(repo).context("failed to determine current branch")?;

    // Load tasks and resolve DAG
    let tasks_dir = repo.join(".rloop").join("tasks");
    let tasks = load_all_tasks(&tasks_dir)?;
    let plan = resolve_dag(&tasks, target_task)?;

    let task_by_id = |id: TaskId| -> Option<&Task> { tasks.iter().find(|t| t.id == id) };

    // Create session branch off current HEAD
    let session_branch = format!("rloop-{target_task:02}");
    create_session_branch(repo, &session_branch)
        .with_context(|| format!("failed to create session branch {session_branch}"))?;

    // Execute each task in DAG order
    let mut completed_tasks: Vec<TaskId> = Vec::new();
    let mut failed_tasks: Vec<TaskId> = Vec::new();
    let mut skipped_tasks: Vec<TaskId> = Vec::new();
    let mut failed_set: HashSet<TaskId> = HashSet::new();

    for &task_id in &plan.steps {
        let task = task_by_id(task_id)
            .with_context(|| format!("task {task_id} in plan but not loaded"))?;

        if task.completed {
            continue;
        }

        let blocked = task.depends_on.iter().any(|dep| failed_set.contains(dep));
        if blocked {
            skipped_tasks.push(task_id);
            emit(
                &mut logger,
                &Event::TaskFailed {
                    task_id,
                    reason: "blocked by failed dependency".to_string(),
                },
            );
            failed_set.insert(task_id);
            continue;
        }

        let outcome =
            execute_task(repo, task, &session_branch, config, &mut logger, executor).await?;

        match outcome {
            TaskOutcome::Success => completed_tasks.push(task_id),
            TaskOutcome::Failed => {
                failed_tasks.push(task_id);
                failed_set.insert(task_id);
            }
        }
    }

    // Auto-merge into parent if requested and everything succeeded
    let did_auto_merge = finalize_session(
        repo,
        &session_branch,
        &parent_branch,
        auto_merge,
        &failed_tasks,
        &skipped_tasks,
        &mut logger,
    )?;

    Ok(SessionResult {
        session_id,
        branch: session_branch,
        completed_tasks,
        failed_tasks,
        skipped_tasks,
        auto_merged: did_auto_merge,
    })
}

/// Run all incomplete tasks in DAG order with the default (real) executor.
///
/// # Errors
/// Returns an error if session initialization, task loading, DAG resolution, or execution fails.
pub async fn run_all_session(
    repo: impl AsRef<Path>,
    config: &Config,
    auto_merge: bool,
) -> anyhow::Result<SessionResult> {
    run_all_session_with(repo, config, auto_merge, &RealStepExecutor).await
}

/// Run all incomplete tasks in DAG order with a custom executor (for testing).
///
/// # Errors
/// Returns an error if session initialization, task loading, DAG resolution, or execution fails.
pub async fn run_all_session_with<E: StepExecutor>(
    repo: impl AsRef<Path>,
    config: &Config,
    auto_merge: bool,
    executor: &E,
) -> anyhow::Result<SessionResult> {
    let repo = repo.as_ref();

    git::assert_has_commits(repo)?;
    git::assert_clean_worktree(repo)?;

    let session_id = SessionId(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let session_dir = repo.join(".rloop").join("sessions");
    let mut logger = SessionLogger::new(&session_dir, session_id.clone())?;

    // Use TaskId(u32::MAX) as sentinel for "all tasks"
    let all_target = TaskId(u32::MAX);
    emit(
        &mut logger,
        &Event::SessionStarted {
            session_id: session_id.clone(),
            target: all_target,
        },
    );

    let parent_branch = git::current_branch(repo).context("failed to determine current branch")?;

    // Load tasks and resolve full DAG
    let tasks_dir = repo.join(".rloop").join("tasks");
    let tasks = load_all_tasks(&tasks_dir)?;
    let plan = resolve_all_dag(&tasks)?;

    if plan.steps.is_empty() {
        emit(
            &mut logger,
            &Event::SessionComplete {
                branch: parent_branch.clone(),
            },
        );
        return Ok(SessionResult {
            session_id,
            branch: parent_branch,
            completed_tasks: Vec::new(),
            failed_tasks: Vec::new(),
            skipped_tasks: Vec::new(),
            auto_merged: false,
        });
    }

    let task_by_id = |id: TaskId| -> Option<&Task> { tasks.iter().find(|t| t.id == id) };

    // Create session branch
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let session_branch = format!("rloop-all-{timestamp}");
    create_session_branch(repo, &session_branch)
        .with_context(|| format!("failed to create session branch {session_branch}"))?;

    // Execute each task in DAG order
    let mut completed_tasks: Vec<TaskId> = Vec::new();
    let mut failed_tasks: Vec<TaskId> = Vec::new();
    let mut skipped_tasks: Vec<TaskId> = Vec::new();
    let mut failed_set: HashSet<TaskId> = HashSet::new();

    for &task_id in &plan.steps {
        let task = task_by_id(task_id)
            .with_context(|| format!("task {task_id} in plan but not loaded"))?;

        let blocked = task.depends_on.iter().any(|dep| failed_set.contains(dep));
        if blocked {
            skipped_tasks.push(task_id);
            emit(
                &mut logger,
                &Event::TaskFailed {
                    task_id,
                    reason: "blocked by failed dependency".to_string(),
                },
            );
            failed_set.insert(task_id);
            continue;
        }

        let outcome =
            execute_task(repo, task, &session_branch, config, &mut logger, executor).await?;

        match outcome {
            TaskOutcome::Success => completed_tasks.push(task_id),
            TaskOutcome::Failed => {
                failed_tasks.push(task_id);
                failed_set.insert(task_id);
            }
        }
    }

    let did_auto_merge = finalize_session(
        repo,
        &session_branch,
        &parent_branch,
        auto_merge,
        &failed_tasks,
        &skipped_tasks,
        &mut logger,
    )?;

    Ok(SessionResult {
        session_id,
        branch: session_branch,
        completed_tasks,
        failed_tasks,
        skipped_tasks,
        auto_merged: did_auto_merge,
    })
}

/// Handle end-of-session: auto-merge or leave session branch for review.
fn finalize_session(
    repo: &Path,
    session_branch: &str,
    parent_branch: &str,
    auto_merge: bool,
    failed_tasks: &[TaskId],
    skipped_tasks: &[TaskId],
    logger: &mut SessionLogger,
) -> anyhow::Result<bool> {
    if auto_merge && failed_tasks.is_empty() && skipped_tasks.is_empty() {
        // Fast-forward parent to session tip, then clean up session branch
        git::git(repo, &["checkout", parent_branch])
            .with_context(|| format!("failed to checkout {parent_branch}"))?;
        git::git(repo, &["merge", "--ff-only", session_branch]).with_context(|| {
            format!("failed to auto-merge {session_branch} into {parent_branch}")
        })?;
        git::git(repo, &["branch", "-d", session_branch])
            .with_context(|| format!("failed to delete session branch {session_branch}"))?;

        emit(
            logger,
            &Event::SessionAutoMerged {
                branch: session_branch.to_string(),
                into_branch: parent_branch.to_string(),
            },
        );
        Ok(true)
    } else {
        // Leave session branch for review — main working tree untouched
        emit(
            logger,
            &Event::SessionComplete {
                branch: session_branch.to_string(),
            },
        );
        Ok(false)
    }
}
