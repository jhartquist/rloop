use std::path::Path;

use anyhow::Context;

use crate::builtin_tool::BuiltinTool;
use crate::git;
use crate::log::{Event, EventSender};
use crate::model::Model;
use crate::query::{QueryConfig, run_query};
use crate::task::Task;

/// Attempt to resolve merge conflicts using a lightweight LLM agent.
///
/// Starts a `--no-commit` merge in the worktree, invokes a Sonnet agent to
/// resolve conflict markers, and verifies the result. Returns `true` if the
/// merge was resolved cleanly, `false` if the agent could not resolve.
///
/// On failure, the merge is aborted and the worktree is left clean.
///
/// # Errors
/// Returns an error if git commands or the LLM query fail for infrastructure
/// reasons (not for conflict resolution failures, which return `Ok(false)`).
async fn attempt_merge_fix(
    worktree_path: &Path,
    task: &Task,
    session_branch: &str,
    attempt: u32,
    logger: &EventSender,
) -> anyhow::Result<bool> {
    let task_id = task.id;

    logger.send(Event::MergeFixStarted { task_id, attempt });

    // Start a --no-commit merge to get conflict markers in the worktree
    let clean = git::merge_no_commit(worktree_path, session_branch)
        .context("failed to start merge for merge-fix agent")?;

    if clean {
        // No conflicts — just commit the merge and return success
        git::git(worktree_path, &["commit", "--no-edit"])
            .context("failed to commit clean merge")?;

        logger.send(Event::MergeFixSucceeded {
            task_id,
            attempt,
            cost_usd: 0.0,
        });
        return Ok(true);
    }

    // Build the merge-fix prompt
    let prompt = build_merge_fix_prompt(task, worktree_path);

    let query_config = QueryConfig {
        model: Model::Sonnet,
        cwd: worktree_path.to_path_buf(),
        tools: vec![
            BuiltinTool::Read,
            BuiltinTool::Edit,
            BuiltinTool::Bash,
            BuiltinTool::Glob,
            BuiltinTool::Grep,
        ],
        output_format: None,
    };

    let result = run_query(&prompt, &query_config).await;

    match result {
        Ok(query_result) => {
            // Forward agent events to the logger
            for event in query_result.events {
                logger.send(event);
            }

            // Check if conflict markers remain
            let has_conflicts = git::has_conflict_markers(worktree_path)
                .context("failed to check for conflict markers")?;

            if has_conflicts {
                git::merge_abort(worktree_path)
                    .context("failed to abort merge after merge-fix agent")?;

                logger.send(Event::MergeFixFailed {
                    task_id,
                    attempt,
                    reason: "conflict markers remain after agent resolution attempt".into(),
                });
                return Ok(false);
            }

            // Stage and commit the resolved merge
            git::git(worktree_path, &["add", "-A"]).context("failed to stage resolved files")?;
            let msg = format!("merge-fix: resolve conflicts for task {task_id:02}");
            git::git(worktree_path, &["commit", "-m", &msg])
                .context("failed to commit merge resolution")?;

            logger.send(Event::MergeFixSucceeded {
                task_id,
                attempt,
                cost_usd: query_result.cost_usd,
            });
            Ok(true)
        }
        Err(e) => {
            // Agent infrastructure failure — abort merge and report
            if let Err(abort_err) = git::merge_abort(worktree_path) {
                eprintln!("warning: failed to abort merge for task {task_id}: {abort_err}");
            }

            logger.send(Event::MergeFixFailed {
                task_id,
                attempt,
                reason: format!("merge-fix agent error: {e:#}"),
            });
            Ok(false)
        }
    }
}

/// Build the prompt for the merge-fix agent.
fn build_merge_fix_prompt(task: &Task, worktree_path: &Path) -> String {
    // Get the list of conflicted files for context
    let diff_output =
        git::git(worktree_path, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();

    let conflicted_files = if diff_output.is_empty() {
        "Unable to determine conflicted files.".to_string()
    } else {
        format!("Conflicted files:\n{diff_output}")
    };

    let title = if task.title.is_empty() {
        format!("task {:02}", task.id)
    } else {
        task.title.clone()
    };

    format!(
        r"You are resolving a git merge conflict. During parallel task execution, this task's branch diverged from the session branch. The working directory now contains files with conflict markers (<<<<<<< ======= >>>>>>>).

Task: {title}

Task description:
{description}

{conflicted_files}

Your job: resolve ALL conflict markers so the merge is clean.

Instructions:
1. Read the conflicted files to understand both sides of the conflict
2. Edit each file to resolve the conflicts — keep both sides' intent where possible
3. After resolving, run: git diff --check
4. If git diff --check shows no issues, you're done
5. If conflicts remain, continue resolving

Do NOT run git add or git commit — the system will handle that.
If you cannot resolve the conflicts cleanly, explain why in your response.",
        description = task.description.trim(),
    )
}

/// Perform the three-phase merge: rebase, then merge-fix agent, then fail.
///
/// This is the merge pipeline used after a task completes execution:
/// 1. Attempt rebase onto session branch (fast, no LLM cost)
/// 2. On rebase conflict: abort rebase, invoke merge-fix agent
/// 3. On merge-fix failure: mark as failed, preserve branch
///
/// All git operations run in the task worktree — the main worktree is never modified.
///
/// Returns `Ok(())` on success (session branch updated), `Err` on failure.
///
/// # Errors
/// Returns an error if the merge cannot be completed (either rebase fails
/// and the merge-fix agent cannot resolve, or infrastructure errors occur).
pub async fn merge_with_fix(
    repo: &Path,
    task: &Task,
    task_branch: &str,
    session_branch: &str,
    worktree_path: &Path,
    attempt: u32,
    logger: &EventSender,
) -> anyhow::Result<()> {
    let task_id = task.id;

    // Phase 1: Rebase (runs in the task worktree, not main worktree)
    if let Err(rebase_err) = git::rebase(worktree_path, session_branch) {
        // Rebase failed — abort it
        if let Err(abort_err) = git::rebase_abort(worktree_path) {
            eprintln!("warning: failed to abort rebase for task {task_id}: {abort_err}");
        }

        // Phase 2: Merge-fix agent (runs in the same task worktree)
        let resolved =
            attempt_merge_fix(worktree_path, task, session_branch, attempt, logger).await?;

        if resolved {
            // Fast-forward session branch to the task worktree's HEAD
            git::fast_forward_ref(repo, session_branch, task_branch).with_context(|| {
                format!("failed to fast-forward session branch after merge-fix for task {task_id}")
            })?;

            logger.send(Event::WorktreeMerged {
                task_id,
                attempt,
                into_branch: session_branch.to_string(),
            });

            return Ok(());
        }

        // Phase 3: Fail
        return Err(rebase_err.context("rebase failed and merge-fix agent could not resolve"));
    }

    // Rebase succeeded — fast-forward session branch
    git::fast_forward_ref(repo, session_branch, task_branch)
        .with_context(|| format!("failed to fast-forward session branch for task {task_id}"))?;

    logger.send(Event::WorktreeMerged {
        task_id,
        attempt,
        into_branch: session_branch.to_string(),
    });

    Ok(())
}
