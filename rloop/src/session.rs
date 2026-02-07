use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::git::{
    self, Worktree, cleanup_worktree, create_worktree, fast_forward_ref, lock_worktree,
};
use crate::lock::SessionLock;
use crate::log::{Event, SessionLogger};
use crate::step::{RealStepExecutor, StepExecutor, StepOutcome, StepResult};
use crate::task::{Task, TaskId, load_all_tasks, resolve_all_dag, resolve_dag};

/// Unique identifier for a session (one invocation of rloop).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Accumulated metrics across steps and tasks.
#[derive(Clone, Debug, Default)]
pub struct SessionMetrics {
    pub duration: Duration,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
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
    pub metrics: SessionMetrics,
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
    if let Err(e) = logger.log(event) {
        eprintln!("warning: failed to write session log: {e}");
    }
}

// ---------------------------------------------------------------------------
// DAG execution (common loop for single-target and all-task sessions)
// ---------------------------------------------------------------------------

/// Result of executing a DAG of tasks (before finalization).
struct DagResult {
    completed_tasks: Vec<TaskId>,
    failed_tasks: Vec<TaskId>,
    skipped_tasks: Vec<TaskId>,
    metrics: SessionMetrics,
}

/// Execute tasks in DAG order, accumulating metrics.
async fn execute_dag<E: StepExecutor>(
    repo: &Path,
    tasks: &[Task],
    steps: &[TaskId],
    session_branch: &str,
    config: &Config,
    logger: &mut SessionLogger,
    executor: &E,
) -> anyhow::Result<DagResult> {
    let task_by_id = |id: TaskId| -> Option<&Task> { tasks.iter().find(|t| t.id == id) };

    let session_start = Instant::now();
    let mut completed_tasks: Vec<TaskId> = Vec::new();
    let mut failed_tasks: Vec<TaskId> = Vec::new();
    let mut skipped_tasks: Vec<TaskId> = Vec::new();
    let mut failed_set: HashSet<TaskId> = HashSet::new();
    let mut metrics = SessionMetrics::default();

    for &task_id in steps {
        let task = task_by_id(task_id)
            .with_context(|| format!("task {task_id} in plan but not loaded"))?;

        if task.completed {
            continue;
        }

        let blocked = task.depends_on.iter().any(|dep| failed_set.contains(dep));
        if blocked {
            skipped_tasks.push(task_id);
            emit(
                logger,
                &Event::TaskFailed {
                    task_id,
                    reason: "blocked by failed dependency".to_string(),
                },
            );
            failed_set.insert(task_id);
            continue;
        }

        let outcome = execute_task(repo, task, session_branch, config, logger, executor).await?;

        match outcome {
            TaskOutcome::Success(ref tm) | TaskOutcome::Failed(ref tm) => {
                metrics.cost_usd += tm.cost_usd;
                metrics.input_tokens += tm.input_tokens;
                metrics.output_tokens += tm.output_tokens;
                metrics.cache_read_tokens += tm.cache_read_tokens;
                metrics.cache_creation_tokens += tm.cache_creation_tokens;
            }
        }
        match outcome {
            TaskOutcome::Success(_) => completed_tasks.push(task_id),
            TaskOutcome::Failed(_) => {
                failed_tasks.push(task_id);
                failed_set.insert(task_id);
            }
        }
    }
    metrics.duration = session_start.elapsed();

    Ok(DagResult {
        completed_tasks,
        failed_tasks,
        skipped_tasks,
        metrics,
    })
}

// ---------------------------------------------------------------------------
// Task execution (single task within a session)
// ---------------------------------------------------------------------------

/// Metrics accumulated for a single task across all retry steps.
#[derive(Clone, Debug, Default)]
struct TaskMetrics {
    duration: Duration,
    cost_usd: f64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    steps: u32,
}

impl TaskMetrics {
    fn add_step(&mut self, step: &StepResult) {
        self.cost_usd += step.cost_usd;
        self.input_tokens += step.input_tokens;
        self.output_tokens += step.output_tokens;
        self.cache_read_tokens += step.cache_read_tokens;
        self.cache_creation_tokens += step.cache_creation_tokens;
        self.steps += 1;
    }
}

/// Outcome of running a single task within a session (including retries).
enum TaskOutcome {
    Success(TaskMetrics),
    Failed(TaskMetrics),
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
    let task_start = Instant::now();

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
    let (summary, mut task_metrics) =
        run_with_retries(task, &worktree, config, logger, executor).await?;
    task_metrics.duration = task_start.elapsed();

    let success = summary.is_some();

    emit(
        logger,
        &Event::TaskSummary {
            task_id,
            success,
            duration: task_metrics.duration,
            cost_usd: task_metrics.cost_usd,
            input_tokens: task_metrics.input_tokens,
            output_tokens: task_metrics.output_tokens,
            steps: task_metrics.steps,
        },
    );

    if let Some(summary) = summary {
        finish_successful_task(repo, &worktree, session_branch, task_id, &summary, logger)?;
        Ok(TaskOutcome::Success(task_metrics))
    } else {
        finish_failed_task(repo, &worktree, task_id, logger)?;
        Ok(TaskOutcome::Failed(task_metrics))
    }
}

/// Retry loop: run `execute_step` up to `max_retries` times, accumulating learnings.
///
/// Returns the completion summary on success (or `None` on failure) along with
/// accumulated task metrics.
async fn run_with_retries<E: StepExecutor>(
    task: &Task,
    worktree: &Worktree,
    config: &Config,
    logger: &mut SessionLogger,
    executor: &E,
) -> anyhow::Result<(Option<String>, TaskMetrics)> {
    let task_id = task.id;
    let mut attempts: u32 = 0;
    let mut prior_learnings: Vec<String> = Vec::new();
    let mut task_metrics = TaskMetrics::default();

    loop {
        attempts += 1;
        emit(logger, &Event::PromptSent { task_id });

        let step_result = executor
            .execute(task, &worktree.path, config, &prior_learnings)
            .await?;

        // Log all Claude stream events from this step
        for event in &step_result.events {
            if let Err(e) = logger.log(event) {
                eprintln!("warning: failed to write stream event to session log: {e}");
            }
        }

        task_metrics.add_step(&step_result);

        // Emit step summary
        emit(
            logger,
            &Event::StepSummary {
                task_id,
                step_number: attempts,
                max_steps: config.max_retries,
                duration: step_result.duration,
                cost_usd: step_result.cost_usd,
                input_tokens: step_result.input_tokens,
                output_tokens: step_result.output_tokens,
            },
        );

        match &step_result.outcome {
            StepOutcome::Success { summary } => {
                emit(
                    logger,
                    &Event::TaskCompleted {
                        task_id,
                        summary: summary.clone(),
                    },
                );
                return Ok((Some(summary.clone()), task_metrics));
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
                    return Ok((None, task_metrics));
                }
            }
        }
    }
}

/// Auto-commit worktree changes, fast-forward the session branch, and clean up.
fn finish_successful_task(
    repo: &Path,
    worktree: &Worktree,
    session_branch: &str,
    task_id: TaskId,
    summary: &str,
    logger: &mut SessionLogger,
) -> anyhow::Result<()> {
    // Commit any changes the agent made in the worktree
    let status = git::git(&worktree.path, &["status", "--porcelain"])?;
    if !status.is_empty() {
        git::git(&worktree.path, &["add", "-A"])?;
        let msg = format!("task {task_id:02}: {summary}");
        git::git(&worktree.path, &["commit", "-m", &msg])?;
    }

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

    let rloop_dir = repo.join(".rloop");
    let _lock = SessionLock::acquire(&rloop_dir).context("failed to acquire session lock")?;

    let session_id = SessionId(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let session_dir = rloop_dir.join("sessions");
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
    let tasks_dir = rloop_dir.join("tasks");
    let tasks = load_all_tasks(&tasks_dir)?;
    let plan = resolve_dag(&tasks, target_task)?;

    // Create session branch off current HEAD
    let session_branch = format!("rloop-{target_task:02}");
    create_session_branch(repo, &session_branch)
        .with_context(|| format!("failed to create session branch {session_branch}"))?;

    let dag_result = execute_dag(
        repo,
        &tasks,
        &plan.steps,
        &session_branch,
        config,
        &mut logger,
        executor,
    )
    .await?;

    // Auto-merge into parent if requested and everything succeeded
    let did_auto_merge = finalize_session(
        repo,
        &session_branch,
        &parent_branch,
        auto_merge,
        &dag_result.failed_tasks,
        &dag_result.skipped_tasks,
        &mut logger,
    )?;

    emit(
        &mut logger,
        &Event::SessionSummary {
            session_id: session_id.clone(),
            duration: dag_result.metrics.duration,
            cost_usd: dag_result.metrics.cost_usd,
            input_tokens: dag_result.metrics.input_tokens,
            output_tokens: dag_result.metrics.output_tokens,
            cache_read_tokens: dag_result.metrics.cache_read_tokens,
            cache_creation_tokens: dag_result.metrics.cache_creation_tokens,
        },
    );

    // Lock released on drop
    Ok(SessionResult {
        session_id,
        branch: session_branch,
        completed_tasks: dag_result.completed_tasks,
        failed_tasks: dag_result.failed_tasks,
        skipped_tasks: dag_result.skipped_tasks,
        auto_merged: did_auto_merge,
        metrics: dag_result.metrics,
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

    let rloop_dir = repo.join(".rloop");
    let _lock = SessionLock::acquire(&rloop_dir).context("failed to acquire session lock")?;

    let session_id = SessionId(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let session_dir = rloop_dir.join("sessions");
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
    let tasks_dir = rloop_dir.join("tasks");
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
            metrics: SessionMetrics::default(),
        });
    }

    // Create session branch
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let session_branch = format!("rloop-all-{timestamp}");
    create_session_branch(repo, &session_branch)
        .with_context(|| format!("failed to create session branch {session_branch}"))?;

    let dag_result = execute_dag(
        repo,
        &tasks,
        &plan.steps,
        &session_branch,
        config,
        &mut logger,
        executor,
    )
    .await?;

    let did_auto_merge = finalize_session(
        repo,
        &session_branch,
        &parent_branch,
        auto_merge,
        &dag_result.failed_tasks,
        &dag_result.skipped_tasks,
        &mut logger,
    )?;

    emit(
        &mut logger,
        &Event::SessionSummary {
            session_id: session_id.clone(),
            duration: dag_result.metrics.duration,
            cost_usd: dag_result.metrics.cost_usd,
            input_tokens: dag_result.metrics.input_tokens,
            output_tokens: dag_result.metrics.output_tokens,
            cache_read_tokens: dag_result.metrics.cache_read_tokens,
            cache_creation_tokens: dag_result.metrics.cache_creation_tokens,
        },
    );

    Ok(SessionResult {
        session_id,
        branch: session_branch,
        completed_tasks: dag_result.completed_tasks,
        failed_tasks: dag_result.failed_tasks,
        skipped_tasks: dag_result.skipped_tasks,
        auto_merged: did_auto_merge,
        metrics: dag_result.metrics,
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
