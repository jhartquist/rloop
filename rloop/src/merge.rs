use std::path::Path;

use anyhow::Context;

use crate::config::Config;
use crate::git;
use crate::log::{Event, EventSender};
use crate::model::Model;
use crate::query::{QueryConfig, run_query};
use crate::task::{Task, TaskId};
use crate::builtin_tool::BuiltinTool;

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
    logger: &EventSender,
) -> anyhow::Result<bool> {
    let task_id = task.id;

    logger.send(Event::MergeFixStarted { task_id });

    // Start a --no-commit merge to get conflict markers in the worktree
    let clean = git::merge_no_commit(worktree_path, session_branch)
        .context("failed to start merge for merge-fix agent")?;

    if clean {
        // No conflicts — just commit the merge and return success
        git::git(worktree_path, &["commit", "--no-edit"])
            .context("failed to commit clean merge")?;

        logger.send(Event::MergeFixSucceeded {
            task_id,
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
        r"You are resolving a git merge conflict. During parallel task execution, this task's branch diverged from the session branch. A rebase was attempted but failed due to conflicts. The working directory now contains files with conflict markers (<<<<<<< ======= >>>>>>>).

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
/// Returns `Ok(())` on success (session branch updated), `Err` on failure.
///
/// # Errors
/// Returns an error if the merge cannot be completed (either rebase fails
/// and the merge-fix agent cannot resolve, or infrastructure errors occur).
#[allow(clippy::too_many_arguments)]
pub async fn merge_with_fix(
    repo: &Path,
    task: &Task,
    task_branch: &str,
    session_branch: &str,
    parent_branch: &str,
    _config: &Config,
    logger: &EventSender,
) -> anyhow::Result<()> {
    let task_id = task.id;

    // Phase 1: Rebase
    if let Err(rebase_err) = git::rebase(repo, task_branch, session_branch) {
        // Rebase failed — abort it
        if let Err(abort_err) = git::rebase_abort(repo) {
            eprintln!("warning: failed to abort rebase for task {task_id}: {abort_err}");
        }
        // Restore main worktree to parent branch after failed rebase
        let _ = git::git(repo, &["checkout", parent_branch]);

        // Phase 2: Merge-fix agent
        // The task's worktree was already cleaned up before merge, so create
        // a temporary one branched off the task branch.
        let temp_wt = create_temp_merge_worktree(repo, task_id, task_branch)?;

        let resolved = attempt_merge_fix(&temp_wt, task, session_branch, logger).await?;

        if resolved {
            // Get the merge-fix worktree's branch tip
            let tip = git::git(&temp_wt, &["rev-parse", "HEAD"])
                .context("failed to get merge-fix worktree HEAD")?;

            // Clean up temp worktree before updating refs
            cleanup_temp_merge_worktree(repo, &temp_wt, task_id);

            // Fast-forward session branch to the merged tip
            git::fast_forward_ref(repo, session_branch, &tip).with_context(|| {
                format!("failed to fast-forward session branch after merge-fix for task {task_id}")
            })?;

            // Restore parent branch checkout
            let _ = git::git(repo, &["checkout", parent_branch]);

            logger.send(Event::WorktreeMerged {
                task_id,
                into_branch: session_branch.to_string(),
            });

            return Ok(());
        }

        // Clean up temp worktree on failure
        cleanup_temp_merge_worktree(repo, &temp_wt, task_id);

        // Phase 3: Fail
        return Err(rebase_err.context("rebase failed and merge-fix agent could not resolve"));
    }

    // Rebase succeeded — fast-forward session branch
    git::fast_forward_ref(repo, session_branch, task_branch)
        .with_context(|| format!("failed to fast-forward session branch for task {task_id}"))?;

    // Restore main worktree to parent branch
    git::git(repo, &["checkout", parent_branch]).with_context(|| {
        format!("failed to restore {parent_branch} after merging task {task_id}")
    })?;

    logger.send(Event::WorktreeMerged {
        task_id,
        into_branch: session_branch.to_string(),
    });

    Ok(())
}

/// Create a temporary worktree for the merge-fix agent.
///
/// Branches off the task branch so the agent has the task's work as a starting point.
fn create_temp_merge_worktree(
    repo: &Path,
    task_id: TaskId,
    task_branch: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let label = format!("{task_id:02}-mergefix");
    let worktree_dir = repo.join(".rloop").join("worktrees");
    std::fs::create_dir_all(&worktree_dir)?;

    let wt_path = worktree_dir.join(&label);
    let branch = format!("rloop-mergefix-{task_id:02}");

    git::git(
        repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().context("non-UTF-8 path")?,
            "-b",
            &branch,
            task_branch,
        ],
    )
    .with_context(|| format!("failed to create merge-fix worktree for task {task_id}"))?;

    Ok(wt_path)
}

/// Clean up a temporary merge-fix worktree (best-effort).
fn cleanup_temp_merge_worktree(repo: &Path, wt_path: &Path, task_id: TaskId) {
    let wt_str = wt_path.to_str().unwrap_or("");
    let branch = format!("rloop-mergefix-{task_id:02}");

    if let Err(e) = git::git(repo, &["worktree", "remove", wt_str, "--force"]) {
        eprintln!("warning: failed to remove merge-fix worktree for task {task_id}: {e}");
    }
    if let Err(e) = git::git(repo, &["branch", "-D", &branch]) {
        eprintln!("warning: failed to delete merge-fix branch for task {task_id}: {e}");
    }
}
