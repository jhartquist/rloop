use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as TokioMutex;

use crate::config::Config;
use crate::git::{
    self, Worktree, cleanup_worktree, cleanup_worktree_preserve_branch, create_worktree,
    lock_worktree,
};
use crate::lock::SessionLock;
use crate::log::{Event, EventSender, SessionLogger, spawn_logger};
use crate::merge;
use crate::step::{RealStepExecutor, StepExecutor, StepOutcome, StepResult};
use crate::task::{
    Task, TaskId, find_ready_tasks, load_all_tasks, mark_completed, resolve_all_dag, resolve_dag,
};

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

fn emit(logger: &EventSender, event: Event) {
    logger.send(event);
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

/// Outcome of a single task's execution phase (before merge).
struct ExecutionResult {
    task_id: TaskId,
    worktree: Worktree,
    outcome: TaskOutcome,
}

/// Execute tasks in DAG order with parallel scheduling.
///
/// Uses `FuturesUnordered` with a semaphore to run up to `max_parallel` tasks
/// concurrently. Merges are serialized via a tokio mutex: rebase-then-fast-forward.
/// With `max_parallel=1`, this degenerates to sequential execution.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn execute_dag<E: StepExecutor>(
    repo: &Path,
    tasks: &[Task],
    steps: &[TaskId],
    session_branch: &str,
    parent_branch: &str,
    config: &Config,
    logger: &EventSender,
    executor: Arc<E>,
) -> anyhow::Result<DagResult> {
    let session_start = Instant::now();
    let mut completed_tasks: Vec<TaskId> = Vec::new();
    let mut failed_tasks: Vec<TaskId> = Vec::new();
    let mut skipped_tasks: Vec<TaskId> = Vec::new();
    let mut completed_set: HashSet<TaskId> = HashSet::new();
    let mut failed_set: HashSet<TaskId> = HashSet::new();
    let mut in_flight: HashSet<TaskId> = HashSet::new();
    let mut metrics = SessionMetrics::default();

    // Pre-populate completed_set with tasks already marked completed
    for task in tasks {
        if task.completed {
            completed_set.insert(task.id);
        }
    }

    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_parallel));
    let merge_mutex = Arc::new(TokioMutex::new(()));
    let mut futures: FuturesUnordered<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<ExecutionResult>> + Send>,
        >,
    > = FuturesUnordered::new();

    loop {
        // Fill the frontier up to max_parallel
        let ready = find_ready_tasks(tasks, steps, &completed_set, &failed_set, &in_flight);
        for task_id in ready {
            if in_flight.len() >= config.max_parallel {
                break;
            }

            let task = tasks
                .iter()
                .find(|t| t.id == task_id)
                .with_context(|| format!("task {task_id} in plan but not loaded"))?;

            in_flight.insert(task_id);

            // Clone/copy what the spawned future needs
            let sem = Arc::clone(&semaphore);
            let repo = repo.to_path_buf();
            let task_id = task.id;
            let task = task.clone();
            let session_branch = session_branch.to_string();
            let config = config.clone();
            let logger = logger.clone();
            let executor = Arc::clone(&executor);

            futures.push(Box::pin(async move {
                // Acquire semaphore permit (controls concurrency)
                let _permit = sem
                    .acquire()
                    .await
                    .map_err(|e| anyhow::anyhow!("semaphore closed: {e}"))?;

                let (worktree, outcome) = execute_task_in_worktree(
                    &repo,
                    &task,
                    &session_branch,
                    &config,
                    &logger,
                    executor,
                )
                .await?;

                Ok(ExecutionResult {
                    task_id,
                    worktree,
                    outcome,
                })
            }));
        }

        // If nothing in-flight and no more ready tasks, we're done
        if futures.is_empty() {
            break;
        }

        // Await the next completion
        let result = futures.next().await.expect("futures should not be empty")?;

        let task_id = result.task_id;
        in_flight.remove(&task_id);

        // Accumulate metrics
        match &result.outcome {
            TaskOutcome::Success(tm) | TaskOutcome::Failed(tm) => {
                metrics.cost_usd += tm.cost_usd;
                metrics.input_tokens += tm.input_tokens;
                metrics.output_tokens += tm.output_tokens;
                metrics.cache_read_tokens += tm.cache_read_tokens;
                metrics.cache_creation_tokens += tm.cache_creation_tokens;
            }
        }

        match result.outcome {
            TaskOutcome::Success(_) => {
                // Remove the worktree directory but keep the branch — can't rebase
                // a branch that's checked out in a worktree.
                cleanup_worktree_preserve_branch(repo, &result.worktree)
                    .with_context(|| format!("failed to cleanup worktree for task {task_id}"))?;
                emit(logger, Event::WorktreeCleanedUp { task_id });

                // Look up the task for the merge-fix agent prompt
                let task = tasks
                    .iter()
                    .find(|t| t.id == task_id)
                    .with_context(|| format!("task {task_id} not found for merge"))?;

                // Serialize merge via mutex: rebase -> merge-fix agent -> fail
                let _guard = merge_mutex.lock().await;
                let merge_result = merge::merge_with_fix(
                    repo,
                    task,
                    &result.worktree.branch,
                    session_branch,
                    parent_branch,
                    config,
                    logger,
                )
                .await;

                match merge_result {
                    Ok(()) => {
                        // Clean up the branch now that it's merged
                        if let Err(e) = git::git(repo, &["branch", "-D", &result.worktree.branch]) {
                            eprintln!(
                                "warning: failed to delete branch {}: {e}",
                                result.worktree.branch
                            );
                        }
                        completed_tasks.push(task_id);
                        completed_set.insert(task_id);
                    }
                    Err(merge_err) => {
                        let reason = format!("{merge_err:#}");
                        emit(
                            logger,
                            Event::MergeFailed {
                                task_id,
                                branch: result.worktree.branch.clone(),
                                reason: reason.clone(),
                            },
                        );
                        emit(
                            logger,
                            Event::TaskFailed {
                                task_id,
                                reason: format!("merge failed: {reason}"),
                            },
                        );
                        // Branch is already preserved (worktree removed above)
                        failed_tasks.push(task_id);
                        failed_set.insert(task_id);
                    }
                }
            }
            TaskOutcome::Failed(_) => {
                cleanup_worktree(repo, &result.worktree).with_context(|| {
                    format!("failed to cleanup worktree for failed task {task_id}")
                })?;
                emit(logger, Event::WorktreeCleanedUp { task_id });
                failed_tasks.push(task_id);
                failed_set.insert(task_id);
            }
        }

        // Check for newly-skipped tasks (dependents of failed tasks)
        for &step_id in steps {
            if completed_set.contains(&step_id)
                || failed_set.contains(&step_id)
                || in_flight.contains(&step_id)
            {
                continue;
            }
            let task = tasks.iter().find(|t| t.id == step_id);
            if let Some(task) = task
                && task.depends_on.iter().any(|dep| failed_set.contains(dep))
            {
                skipped_tasks.push(step_id);
                emit(
                    logger,
                    Event::TaskFailed {
                        task_id: step_id,
                        reason: "blocked by failed dependency".to_string(),
                    },
                );
                failed_set.insert(step_id);
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

/// Execute a single task in a worktree without merging. Returns the worktree
/// and outcome so the caller can serialize merges.
async fn execute_task_in_worktree<E: StepExecutor>(
    repo: &Path,
    task: &Task,
    session_branch: &str,
    config: &Config,
    logger: &EventSender,
    executor: Arc<E>,
) -> anyhow::Result<(Worktree, TaskOutcome)> {
    let task_id = task.id;
    let task_start = Instant::now();

    emit(logger, Event::TaskStarted { task_id });

    // Create worktree off session branch
    let worktree = create_worktree(repo, task_id, session_branch)
        .with_context(|| format!("failed to create worktree for task {task_id}"))?;

    emit(
        logger,
        Event::WorktreeCreated {
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
        run_with_retries(task, &worktree, config, logger, &executor).await?;
    task_metrics.duration = task_start.elapsed();

    let success = summary.is_some();

    emit(
        logger,
        Event::TaskSummary {
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
        // Auto-commit worktree changes (but don't merge yet)
        let task_file = worktree.path.join(format!(".rloop/tasks/{task_id:02}.md"));
        mark_completed(&task_file)
            .with_context(|| format!("failed to mark task {task_id} as completed"))?;

        git::git(&worktree.path, &["add", "-A"])?;
        let msg = format!("task {task_id:02}: {summary}");
        git::git(&worktree.path, &["commit", "-m", &msg])?;

        emit(logger, Event::TaskCompleted { task_id, summary });
        Ok((worktree, TaskOutcome::Success(task_metrics)))
    } else {
        Ok((worktree, TaskOutcome::Failed(task_metrics)))
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
    logger: &EventSender,
    executor: &E,
) -> anyhow::Result<(Option<String>, TaskMetrics)> {
    let task_id = task.id;
    let mut attempts: u32 = 0;
    let mut prior_learnings: Vec<String> = Vec::new();
    let mut task_metrics = TaskMetrics::default();

    loop {
        attempts += 1;
        emit(logger, Event::PromptSent { task_id });

        let step_result = executor
            .execute(task, &worktree.path, config, &prior_learnings)
            .await?;

        // Emit step start info (model_id and cc_session_id come from the SDK System message)
        emit(
            logger,
            Event::StepStarted {
                task_id,
                attempt: attempts,
                model_id: step_result
                    .model_id
                    .clone()
                    .unwrap_or_else(|| step_result.model.to_string()),
                cc_session_id: step_result.cc_session_id.clone().unwrap_or_default(),
            },
        );

        task_metrics.add_step(&step_result);

        // Send all Claude stream events from this step to the logger
        for event in step_result.events {
            logger.send(event);
        }

        // Emit step summary
        emit(
            logger,
            Event::StepSummary {
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
                return Ok((Some(summary.clone()), task_metrics));
            }
            StepOutcome::Failed { reason, learnings } => {
                prior_learnings.push(learnings.clone());

                if attempts >= config.max_retries {
                    emit(
                        logger,
                        Event::TaskFailed {
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
    run_session_with(repo, target_task, config, auto_merge, Arc::new(RealStepExecutor)).await
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
    executor: Arc<E>,
) -> anyhow::Result<SessionResult> {
    let repo = repo.as_ref();

    git::assert_has_commits(repo)?;
    git::assert_clean_worktree(repo)?;

    let rloop_dir = repo.join(".rloop");
    let _lock = SessionLock::acquire(&rloop_dir).context("failed to acquire session lock")?;

    let session_id = SessionId(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let session_dir = rloop_dir.join("sessions");
    let session_logger = SessionLogger::new(&session_dir, session_id.clone())?;
    let (logger, logger_handle) = spawn_logger(session_logger);

    let parent_branch = git::current_branch(repo).context("failed to determine current branch")?;

    // Load tasks and resolve DAG
    let tasks_dir = rloop_dir.join("tasks");
    let tasks = load_all_tasks(&tasks_dir)?;
    let plan = resolve_dag(&tasks, target_task)?;

    // Create session branch off current HEAD
    let session_branch = format!("rloop-{target_task:02}");

    emit(
        &logger,
        Event::SessionStarted {
            session_id: session_id.clone(),
            target: Some(target_task),
        },
    );

    let datetime = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string();
    emit(
        &logger,
        Event::SessionBanner {
            session_id: session_id.clone(),
            datetime,
            target: Some(target_task),
            parent_branch: parent_branch.clone(),
            session_branch: session_branch.clone(),
        },
    );

    create_session_branch(repo, &session_branch)
        .with_context(|| format!("failed to create session branch {session_branch}"))?;

    let dag_result = execute_dag(
        repo,
        &tasks,
        &plan.steps,
        &session_branch,
        &parent_branch,
        config,
        &logger,
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
        &logger,
    )?;

    emit(
        &logger,
        Event::SessionSummary {
            session_id: session_id.clone(),
            duration: dag_result.metrics.duration,
            cost_usd: dag_result.metrics.cost_usd,
            input_tokens: dag_result.metrics.input_tokens,
            output_tokens: dag_result.metrics.output_tokens,
            cache_read_tokens: dag_result.metrics.cache_read_tokens,
            cache_creation_tokens: dag_result.metrics.cache_creation_tokens,
        },
    );

    // Drop sender to close channel, then await logger task
    drop(logger);
    let _ = logger_handle.await;

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
    run_all_session_with(repo, config, auto_merge, Arc::new(RealStepExecutor)).await
}

/// Run all incomplete tasks in DAG order with a custom executor (for testing).
///
/// # Errors
/// Returns an error if session initialization, task loading, DAG resolution, or execution fails.
#[allow(clippy::too_many_lines)]
pub async fn run_all_session_with<E: StepExecutor>(
    repo: impl AsRef<Path>,
    config: &Config,
    auto_merge: bool,
    executor: Arc<E>,
) -> anyhow::Result<SessionResult> {
    let repo = repo.as_ref();

    git::assert_has_commits(repo)?;
    git::assert_clean_worktree(repo)?;

    let rloop_dir = repo.join(".rloop");
    let _lock = SessionLock::acquire(&rloop_dir).context("failed to acquire session lock")?;

    let session_id = SessionId(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let session_dir = rloop_dir.join("sessions");
    let session_logger = SessionLogger::new(&session_dir, session_id.clone())?;
    let (logger, logger_handle) = spawn_logger(session_logger);

    let parent_branch = git::current_branch(repo).context("failed to determine current branch")?;

    // Load tasks and resolve full DAG
    let tasks_dir = rloop_dir.join("tasks");
    let tasks = load_all_tasks(&tasks_dir)?;
    let plan = resolve_all_dag(&tasks)?;

    // Create session branch
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let session_branch = format!("rloop-all-{timestamp}");

    emit(
        &logger,
        Event::SessionStarted {
            session_id: session_id.clone(),
            target: None,
        },
    );

    let datetime = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string();
    emit(
        &logger,
        Event::SessionBanner {
            session_id: session_id.clone(),
            datetime,
            target: None,
            parent_branch: parent_branch.clone(),
            session_branch: session_branch.clone(),
        },
    );

    if plan.steps.is_empty() {
        emit(
            &logger,
            Event::SessionComplete {
                branch: parent_branch.clone(),
            },
        );
        // Drop sender to close channel, then await logger task
        drop(logger);
        let _ = logger_handle.await;
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

    create_session_branch(repo, &session_branch)
        .with_context(|| format!("failed to create session branch {session_branch}"))?;

    let dag_result = execute_dag(
        repo,
        &tasks,
        &plan.steps,
        &session_branch,
        &parent_branch,
        config,
        &logger,
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
        &logger,
    )?;

    emit(
        &logger,
        Event::SessionSummary {
            session_id: session_id.clone(),
            duration: dag_result.metrics.duration,
            cost_usd: dag_result.metrics.cost_usd,
            input_tokens: dag_result.metrics.input_tokens,
            output_tokens: dag_result.metrics.output_tokens,
            cache_read_tokens: dag_result.metrics.cache_read_tokens,
            cache_creation_tokens: dag_result.metrics.cache_creation_tokens,
        },
    );

    // Drop sender to close channel, then await logger task
    drop(logger);
    let _ = logger_handle.await;

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
    logger: &EventSender,
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
            Event::SessionAutoMerged {
                branch: session_branch.to_string(),
                into_branch: parent_branch.to_string(),
            },
        );
        Ok(true)
    } else {
        // Leave session branch for review — main working tree untouched
        emit(
            logger,
            Event::SessionComplete {
                branch: session_branch.to_string(),
            },
        );
        Ok(false)
    }
}
