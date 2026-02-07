# Research: Parallel DAG Implementation Plan

## Context

Research doc `02-parallel-dag-and-daemon.md` explored the design space for parallel execution and daemon mode. This document captures the **resolved decisions** and provides an implementation-ready specification for parallel DAG execution. Daemon mode is deferred to a future milestone.

The current `execute_dag` in `session.rs` runs tasks sequentially in a `for` loop. Each task gets a worktree branched off the session branch, and on success, `fast_forward_ref` moves the session branch forward. This works because sequential execution guarantees each worktree branch is a direct descendant of the session branch.

With parallel execution, multiple tasks branch off the same session-branch commit. When the second task finishes, its branch diverges from the (now-updated) session branch. This requires a real merge strategy.

---

## Resolved Design Decisions

### Scheduler: FuturesUnordered with Semaphore

Replace the sequential `for` loop with a frontier-based scheduler using `FuturesUnordered`. Tasks whose dependencies are all satisfied form the "frontier" of runnable tasks. The scheduler fills slots up to `max_parallel`, waits for completions, merges results, and discovers newly unblocked tasks.

```rust
loop {
    // Fill frontier slots up to max_parallel
    while in_flight.len() < max_parallel {
        if let Some(task_id) = next_ready_task(...) {
            let permit = semaphore.acquire().await;
            in_flight.push(spawn_task(task_id, ...));
        } else {
            break;
        }
    }
    if in_flight.is_empty() { break; }

    // Wait for next completion, merge result
    let (task_id, outcome) = in_flight.next().await;
    handle_completion(task_id, outcome, ...);
}
```

**Why FuturesUnordered over channels:** Smallest delta from the current sequential loop. One code path — `max_parallel=1` is just a semaphore with 1 permit. When we build daemon mode later, we'll refactor to a channel-based actor model where tasks arrive asynchronously.

### max_parallel Config

```toml
# .rloop/config.toml
[session]
max_parallel = 1  # default: sequential, opt-in parallelism
```

Default is 1 to preserve current behavior. Users set 2-3 when ready. The config field lives in a new `[session]` section (or directly on `Config` — TBD based on how many session-related fields we accumulate).

### Worktree Creation: Lazy (Just-in-Time)

Worktrees are created when a task is about to start (after acquiring the semaphore permit), not eagerly for the entire frontier. This means later tasks branch off the most up-to-date session branch, reducing divergence and conflict probability.

Sequence per task:
1. Acquire semaphore permit
2. Create worktree off current session branch HEAD
3. Execute task (retries happen within this worktree)
4. On completion, attempt merge into session branch
5. Release permit, cleanup worktree

### Single Code Path

Always use the parallel scheduler, even for `max_parallel=1`. No dual code paths. The parallel scheduler with a semaphore of 1 degenerates to sequential execution. One path to maintain and test.

### Logger: Channel-Based

Current `SessionLogger` takes `&mut self`. With concurrent tasks, we need shared access without lock contention.

Architecture:
- Spawn a dedicated logger task that owns `SessionLogger`
- Create an `mpsc::Sender<Event>` channel
- Each concurrent task gets a `Sender<Event>` clone
- Logger task receives events and calls `log()` + `print()`
- Channel provides natural serialization of writes

```rust
struct EventSender(mpsc::Sender<Event>);

impl EventSender {
    async fn send(&self, event: Event) { ... }
}

async fn logger_task(mut rx: mpsc::Receiver<Event>, mut logger: SessionLogger) {
    while let Some(event) = rx.recv().await {
        logger.print(&event);
        let _ = logger.log(&event);
    }
}
```

The `execute_dag` function spawns the logger task, distributes senders to task futures, and awaits the logger task after all work completes.

### Error Handling: Same Semantics

Failed tasks go in `failed_set`. Dependents are skipped. Session continues with unblocked tasks. No change from current behavior — just applied at the frontier level instead of the sequential loop level.

When a task fails while other tasks are in-flight: the in-flight tasks continue (they're independent by definition). When they complete, the scheduler checks `failed_set` before scheduling any newly unblocked tasks.

---

## Merge Strategy (Critical Path)

### Three-Phase Merge: Rebase → Merge-Fix Agent → Fail

When a task completes successfully, it must merge into the session branch. This is serialized — only one merge at a time (via a tokio Mutex).

**Phase 1: Rebase**
```
git rebase <session-branch> <worktree-branch>
```
If the worktree branch is already a descendant of the session branch (first task to merge, or no divergence), this is a no-op. The rebase produces a linear history on top of the session branch.

On success: fast-forward the session branch.
```
git branch -f <session-branch> <worktree-branch>
```

**Phase 2: Merge-Fix Agent (on conflict)**
If rebase fails with conflicts:
1. Abort the failed rebase: `git rebase --abort`
2. Spawn a merge-fix agent via `run_query`
3. Agent gets: conflict diff, both sides' changes, task descriptions
4. Agent has Read, Edit, Bash tools in the worktree
5. Agent attempts to resolve conflicts and produce a clean merge
6. If clean: commit the resolution, fast-forward session branch
7. If agent fails: proceed to Phase 3

**Phase 3: Mark as Failed**
If the merge-fix agent can't resolve:
- Task is treated as failed (goes in `failed_tasks`)
- Work is preserved on the worktree branch (`rloop-wt-{id}`)
- Worktree directory is cleaned up but **branch is NOT deleted**
- Dependents are skipped
- Session continues with remaining unblocked tasks
- Human can manually recover the branch later

### Merge-Fix Agent Design

The merge-fix agent is a lightweight `run_query` invocation:

```rust
async fn attempt_merge_fix(
    worktree_path: &Path,
    task: &Task,
    session_branch: &str,
    config: &Config,
) -> anyhow::Result<bool>  // true = resolved, false = gave up
```

**Setup:**
1. In the worktree, start a merge (not rebase) to get conflict markers:
   ```
   git merge <session-branch> --no-commit
   ```
2. This leaves conflict markers in the working tree

**Agent prompt:**
```
You are resolving a git merge conflict. The working directory contains
files with conflict markers (<<<<<<< ======= >>>>>>>).

Task that produced these changes: {task.description}

Your job: resolve all conflict markers so the merge is clean.
- Read the conflicted files
- Understand both sides
- Edit to resolve (keep both sides' intent where possible)
- Run: git add -A && git diff --check (verify no remaining markers)

If you cannot resolve cleanly, say so in your summary.
```

**Tools:** Read, Edit, Bash, Glob, Grep (same as a regular task, minus the MCP complete/context_usage tools — this uses `run_query` which handles completion via structured output or final message).

**Model:** Sonnet (sufficient for merge resolution, cheaper than Opus).

**Outcome:**
- Agent resolves → commit the merge, fast-forward session branch
- Agent fails → abort merge, mark task as failed

### Preserving Failed Merge Branches

When a task succeeds execution but fails to merge:
- `cleanup_worktree` removes the worktree directory
- But we **skip the `git branch -D` step** for merge-failed tasks
- The branch `rloop-wt-{id}` persists for manual recovery
- Log a clear message: "Task {id} completed but merge failed. Work preserved on branch rloop-wt-{id}."

This requires a small change to `cleanup_worktree` — add a `preserve_branch: bool` parameter (or a separate cleanup function).

---

## Frontier Detection

The current `resolve_dag` / `resolve_all_dag` returns a flat topological ordering (`Vec<TaskId>`). For parallel execution, we need to identify which tasks in that ordering are currently runnable.

A task is **ready** when:
- It is not completed
- It is not in `failed_set`
- It is not already in-flight
- All its dependencies are either completed or were already completed before the session started

```rust
fn find_ready_tasks(
    tasks: &[Task],
    steps: &[TaskId],
    completed: &HashSet<TaskId>,
    failed: &HashSet<TaskId>,
    in_flight: &HashSet<TaskId>,
) -> Vec<TaskId> {
    steps.iter()
        .filter(|id| !completed.contains(id))
        .filter(|id| !failed.contains(id))
        .filter(|id| !in_flight.contains(id))
        .filter(|id| {
            let task = find_task(tasks, **id);
            task.depends_on.iter().all(|dep| completed.contains(dep))
        })
        .copied()
        .collect()
}
```

This is called after each task completion to discover newly unblocked tasks.

---

## Ctrl+C with Parallel Tasks

Current behavior: `tokio::select!` in `main.rs` catches Ctrl+C, scans `.rloop/worktrees/`, cleans up all directories and branches.

With parallel tasks, multiple worktrees exist simultaneously. The cleanup logic already handles this — it scans the directory and cleans everything. The only addition: in-flight Claude streams need to be interrupted.

When Ctrl+C fires:
1. Drop the `FuturesUnordered` (this drops all in-flight futures, which drops the Claude clients)
2. The existing cleanup scan handles worktree directories and branches
3. Log `SessionCancelled` event

The `tokio::select!` pattern in `main.rs` already handles this correctly — dropping the session future drops all child futures.

---

## StepExecutor Trait Compatibility

The `StepExecutor` trait already has `Send + Sync` bounds and takes `&self`:

```rust
pub trait StepExecutor: Send + Sync {
    fn execute(&self, ...) -> impl Future<Output = Result<StepResult>> + Send;
}
```

This works directly with `Arc<E>` for spawning into multiple futures. No trait changes needed.

---

## Config Changes

Add `max_parallel` to config:

```rust
pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
    pub timeout_seconds: u64,
    pub task: TaskConfig,
    pub max_parallel: usize,  // NEW: default 1
}
```

TOML:
```toml
[session]
max_parallel = 2
```

Or flat at the top level — depends on whether we want a `[session]` section. Given future daemon config will need its own section, a `[session]` section makes sense.

---

## Testing Strategy

### Unit Tests (no Claude API)

- **Frontier detection:** Given a DAG and completed/failed/in-flight sets, verify correct ready tasks
- **Merge serialization:** Mock executor that completes tasks with known file changes, verify session branch has all changes in order
- **Merge conflict handling:** Create two tasks that modify the same file, verify merge-fix is attempted, verify merge-failed handling
- **`max_parallel=1` equivalence:** Verify that the parallel scheduler with max_parallel=1 produces identical results to... itself (since we're removing the old path)
- **Error propagation:** Task fails → dependents skipped, independent tasks continue
- **Branch preservation:** Merge-failed tasks preserve their worktree branch

### Integration Tests (with mock executor)

- **Diamond DAG:** A depends on B and C (independent). B and C run in parallel, A runs after both.
- **Wide DAG:** 5 independent tasks, max_parallel=2. Verify only 2 run at a time.
- **Failure mid-flight:** Task fails while sibling is running. Sibling completes, dependents of failed task are skipped.
- **All-tasks session:** `run_all_session` with parallel execution.

### Manual Testing

- Real Claude execution with 2-3 independent tasks
- Verify terminal output is readable (events from concurrent tasks interleaved)
- Verify session log has all events
- Verify Ctrl+C cleans up all worktrees

---

## What This Does NOT Include

- **Daemon mode** — deferred to future milestone
- **Cloud workers** — deferred
- **TaskStore trait** — not needed yet, extract when daemon needs it
- **Compilation contention mitigation** — document sccache recommendation, don't enforce
- **`parallel_safe` task frontmatter** — not needed, merge strategy handles conflicts
- **Code review integration** — separate feature, not coupled to parallelism
