# Research: Parallel DAG Execution and Daemon Mode

## Context

Today `execute_dag` in `session.rs` runs tasks one at a time in a sequential `for` loop. Each task gets its own git worktree branched off the session branch, and on success, the worktree is fast-forwarded into the session branch before the next task starts. This is simple and correct, but leaves concurrency on the table: independent tasks in the DAG (tasks with no dependency relationship) could run simultaneously.

The goal: evolve rloop from sequential single-task execution to parallel DAG execution (capped at 2-3 concurrent tasks), and eventually to a daemon mode that picks up tasks automatically.

---

## 1. Parallel DAG Execution

### Current Architecture

The sequential loop in `execute_dag` (session.rs:87-154):

```rust
for &task_id in steps {
    // skip completed, skip blocked by failed dependency
    let outcome = execute_task(repo, task, session_branch, config, logger, executor).await?;
    // accumulate metrics, track failures
}
```

Key properties of the current design:
- `steps` is a topological sort — dependencies come before dependents
- Each task branches off the session branch via `create_worktree`
- On success, `fast_forward_ref` moves the session branch forward to the task's branch
- The session logger (`&mut SessionLogger`) is passed mutably — single writer
- `SessionLock` prevents concurrent rloop processes

### What Needs to Change for Parallelism

**Identifying independent tasks.** The topological sort already gives us a dependency-respecting order, but it doesn't tell us which tasks at a given "level" are independent. We need to identify tasks whose dependencies are all satisfied and that don't depend on each other. This is the "frontier" of runnable tasks.

**Worktree branching.** Today each worktree branches off the session branch. With parallel tasks, the session branch doesn't advance until a task merges. Two concurrent tasks would both branch off the same session-branch commit. This is fine as long as:
1. They modify different files (which they should, since they're independent tasks)
2. Merges are serialized (only one task fast-forwards the session branch at a time)

**Merge serialization.** `fast_forward_ref` uses `git branch -f` which is a ref update, not a merge. With parallel tasks, we can't fast-forward — the second task's branch diverges from the first task's branch. We need real merges. Options:
- Sequential merge: when each task completes, take a lock, merge into session branch
- Batch merge: wait for all tasks in a frontier to complete, then merge all at once
- Rebase: rebase each completed task onto the updated session branch, then fast-forward

**Logger concurrency.** `SessionLogger` takes `&mut self` and writes to a `BufWriter<File>`. With concurrent tasks, we need either:
- A shared logger behind `Arc<Mutex<SessionLogger>>`
- Per-task loggers that write to separate files, merged at the end
- A channel-based approach: tasks send events to a logger task

### Approach A: Tokio JoinSet with Semaphore

The most straightforward tokio-native approach.

```rust
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

async fn execute_dag_parallel(
    repo: &Path,
    tasks: &[Task],
    steps: &[TaskId],
    session_branch: &str,
    config: &Config,
    logger: Arc<Mutex<SessionLogger>>,
    executor: Arc<dyn StepExecutor>,
    max_parallel: usize,
) -> Result<DagResult> {
    let semaphore = Arc::new(Semaphore::new(max_parallel));
    let mut join_set = JoinSet::new();

    // Track state
    let completed = Arc::new(Mutex::new(HashSet::new()));
    let failed = Arc::new(Mutex::new(HashSet::new()));
    let merge_lock = Arc::new(tokio::sync::Mutex::new(()));

    // Frontier-based scheduling
    loop {
        let frontier = find_ready_tasks(steps, tasks, &completed, &failed);
        if frontier.is_empty() && join_set.is_empty() {
            break;
        }

        for task_id in frontier {
            let permit = semaphore.clone().acquire_owned().await?;
            join_set.spawn(async move {
                let result = execute_task(...).await;
                drop(permit);
                (task_id, result)
            });
        }

        // Wait for at least one task to complete
        if let Some(result) = join_set.join_next().await {
            let (task_id, outcome) = result??;
            // Serialize the merge
            let _lock = merge_lock.lock().await;
            merge_into_session_branch(...);
        }
    }
}
```

**Trade-offs:**
- (+) Simple concurrency model using tokio primitives
- (+) `Semaphore` naturally caps parallelism
- (+) `JoinSet` tracks all spawned tasks and allows waiting for completion
- (-) Requires `Arc`-wrapping everything shared across tasks (executor, logger, repo path)
- (-) Frontier detection loop needs careful implementation to avoid busy-waiting
- (-) Error handling is more complex — need to decide what happens when one task fails while others are in-flight

**Merge strategy with JoinSet:**
Each task runs independently in its worktree. When a task completes successfully, it takes the `merge_lock`, checks out the session branch, merges the task branch (or rebases and fast-forwards), then releases the lock. This serializes merges while allowing parallel execution.

### Approach B: Channel-Based Actor Model

A more structured approach using channels for coordination.

```rust
enum DagMessage {
    TaskReady(TaskId),
    TaskCompleted(TaskId, TaskMetrics),
    TaskFailed(TaskId, String),
    AllDone,
}

async fn dag_coordinator(
    mut rx: mpsc::Receiver<DagMessage>,
    tx: mpsc::Sender<DagMessage>,
    tasks: &[Task],
    // ...
) {
    let mut in_flight: HashSet<TaskId> = HashSet::new();
    let mut completed: HashSet<TaskId> = HashSet::new();
    let mut failed: HashSet<TaskId> = HashSet::new();

    // Seed initial frontier
    for task_id in find_ready_tasks(tasks, &completed, &failed) {
        tx.send(DagMessage::TaskReady(task_id)).await;
    }

    while let Some(msg) = rx.recv().await {
        match msg {
            DagMessage::TaskCompleted(id, metrics) => {
                in_flight.remove(&id);
                completed.insert(id);
                // Merge into session branch (serialized by being in the coordinator)
                merge_task(...);
                // Find newly unblocked tasks
                for ready in find_newly_ready(tasks, &completed, &failed, &in_flight) {
                    tx.send(DagMessage::TaskReady(ready)).await;
                }
                if in_flight.is_empty() && no_more_tasks() {
                    break;
                }
            }
            DagMessage::TaskFailed(id, reason) => {
                in_flight.remove(&id);
                failed.insert(id);
                // Mark transitive dependents as skipped
            }
            _ => {}
        }
    }
}
```

Workers pull `TaskReady` messages and spawn execution:

```rust
async fn dag_worker(
    rx: mpsc::Receiver<DagMessage>,
    tx: mpsc::Sender<DagMessage>,
    semaphore: Arc<Semaphore>,
) {
    while let Ok(DagMessage::TaskReady(task_id)) = rx.recv().await {
        let permit = semaphore.acquire().await;
        let result = execute_task(task_id, ...).await;
        tx.send(match result {
            Ok(metrics) => DagMessage::TaskCompleted(task_id, metrics),
            Err(e) => DagMessage::TaskFailed(task_id, e.to_string()),
        }).await;
        drop(permit);
    }
}
```

**Trade-offs:**
- (+) Clean separation of scheduling logic (coordinator) from execution (workers)
- (+) Coordinator serializes all state transitions — no shared mutable state
- (+) Natural home for merge serialization (coordinator does it)
- (+) Events from concurrent tasks flow through channels — no lock contention on logger
- (+) Maps naturally to future distributed execution (coordinator = central, workers = remote)
- (-) More infrastructure (channels, message types, coordinator loop)
- (-) Harder to understand at a glance compared to the JoinSet approach
- (-) Need to handle channel closure and backpressure

### Approach C: Bounded Futures with `FuturesUnordered`

A middle ground using `futures::stream::FuturesUnordered`.

```rust
use futures::stream::{FuturesUnordered, StreamExt};

async fn execute_dag_parallel(
    // ...
    max_parallel: usize,
) -> Result<DagResult> {
    let mut in_flight = FuturesUnordered::new();
    let mut completed = HashSet::new();
    let mut failed = HashSet::new();

    loop {
        // Fill up to max_parallel with ready tasks
        while in_flight.len() < max_parallel {
            if let Some(task_id) = find_next_ready(steps, tasks, &completed, &failed, &in_flight_ids) {
                in_flight.push(execute_task(task_id, ...));
            } else {
                break;
            }
        }

        if in_flight.is_empty() {
            break;
        }

        // Wait for the next task to complete
        let (task_id, outcome) = in_flight.next().await.unwrap()?;
        merge_into_session_branch(...);
        // Update completed/failed sets
    }
}
```

**Trade-offs:**
- (+) Most similar to the current sequential loop — easy to understand the evolution
- (+) No channels, no actor model — just a stream of futures
- (+) Merge happens naturally in the main loop after each completion (serialized)
- (-) Less clean separation than the actor model
- (-) `FuturesUnordered` requires the futures to be `'static`, which means `Arc`-wrapping shared state
- (-) Doesn't scale to distributed execution as naturally

### Git Merge Strategy (Critical Detail)

With parallel execution, the simple `fast_forward_ref` no longer works because parallel tasks diverge from the session branch.

**Option 1: Merge commit.** Each completed task is merged into the session branch with a merge commit:
```
git checkout session-branch
git merge rloop-wt-03 --no-edit
```
This creates diamond-shaped merge history. Correct but noisy.

**Option 2: Rebase then fast-forward.** Each completed task is rebased onto the updated session branch, then fast-forwarded:
```
git rebase session-branch rloop-wt-03
git branch -f session-branch rloop-wt-03
```
This maintains linear history. Requires the rebase to succeed (no conflicts). Since independent tasks should modify different files, conflicts should be rare. If a rebase fails, fall back to merge.

**Option 3: Sequential worktree creation.** Instead of creating all worktrees upfront, create each worktree when the task is about to start, branching off the *latest* session branch. If a previous parallel task has already merged, the new worktree includes those changes. This reduces divergence but means worktrees are created just-in-time rather than all at once.

**Recommendation: Option 2 (rebase then fast-forward) with fallback to merge.** Linear history is valuable for human review. Independent tasks rarely conflict. The rebase-then-fast-forward approach preserves the current clean linear history while supporting parallelism.

### Logger Strategy for Concurrent Tasks

The current `SessionLogger` is a single-writer JSONL appender. With concurrent tasks, options:

**Option 1: `Arc<Mutex<SessionLogger>>`.**
Simple but contentious — every event from every concurrent task takes the lock. Since logging is fast (just a write to a buffered file), contention should be low in practice.

**Option 2: Per-task event buffers, merged on completion.**
Each task collects events into a `Vec<Event>`. When the task completes, the events are flushed to the logger in order. This preserves per-task event ordering but interleaves tasks at the task-completion boundary.

**Option 3: Channel-based logger.**
Tasks send events to an `mpsc` channel. A dedicated logger task reads from the channel and writes to the file. This decouples event production from file I/O and naturally serializes writes.

**Recommendation: Option 3 (channel-based logger).** It's clean, scalable, and eliminates any concern about lock contention. The logger task is trivial:

```rust
async fn logger_task(mut rx: mpsc::Receiver<Event>, mut logger: SessionLogger) {
    while let Some(event) = rx.recv().await {
        logger.print(&event);
        if let Err(e) = logger.log(&event) {
            eprintln!("warning: failed to write event: {e}");
        }
    }
}
```

Each concurrent task gets a `tx: mpsc::Sender<Event>` clone.

### Error Handling with Parallel Tasks

When one task in a parallel frontier fails:
1. **Don't cancel in-flight tasks.** Independent tasks should continue — they don't depend on the failed task.
2. **Mark transitive dependents as skipped.** Any task that depends (directly or transitively) on the failed task should be skipped.
3. **Continue scheduling unblocked tasks.** Tasks in the DAG that don't depend on the failed task should proceed.

This is the current behavior for sequential execution (session.rs handles failures by adding to `failed_set` and skipping blocked tasks). The parallel version just needs to do the same check before scheduling each new task.

### Concurrency Cap

The `max_parallel` setting controls how many tasks run simultaneously.

**Default: 2.** Conservative but safe. Two Claude agents plus compilation overlap is manageable on most machines.

**Configurable:**
```toml
# .rloop/config.toml
[session]
max_parallel = 2  # 1 = sequential (current behavior), 2-3 = parallel
```

Setting `max_parallel = 1` preserves the exact current behavior, which is important for debugging and for machines with limited resources.

### Recommended Approach

**Approach C (FuturesUnordered) for v1, with an eye toward Approach B (channels) for daemon mode.**

Rationale:
- FuturesUnordered is the smallest delta from the current sequential loop
- It handles the core requirement (parallel independent tasks with capped concurrency)
- Merges are naturally serialized in the main loop
- The channel-based approach is overkill for v1 but is the right architecture for daemon mode where tasks arrive asynchronously

The evolution path:
1. v1: Sequential (current) — `for task in steps { ... }`
2. v1.1: Parallel with FuturesUnordered — same loop, but fills multiple slots
3. v2: Channel-based coordinator — required for daemon mode's dynamic task arrival

---

## 2. Compilation Contention

### The Problem

Multiple Rust agents compiling in parallel worktrees will fight over:
1. **CPU** — `cargo build` is CPU-intensive and typically uses all cores
2. **Disk I/O** — writing to target directories, running linker
3. **Cargo target directory** — by default each worktree gets its own `target/`, meaning no shared compilation cache
4. **Memory** — parallel `rustc` invocations can use several GB each

### cargo target dir isolation

By default, each git worktree has its own working directory, so `cargo build` creates a separate `target/` in each worktree. This means:
- No shared compilation cache between worktrees
- Common dependencies (tokio, serde, etc.) are compiled separately in each worktree
- Disk usage multiplies linearly with parallel tasks

**Shared target directory.** Setting `CARGO_TARGET_DIR` or `build.target-dir` in `.cargo/config.toml` to a single shared directory:

```toml
# .cargo/config.toml (in main repo, inherited by worktrees)
[build]
target-dir = "/absolute/path/to/shared-target"
```

Trade-offs:
- (+) Dependencies compiled once, shared across worktrees
- (+) Dramatically reduced disk usage and compilation time for dependencies
- (-) Cargo holds file locks on `target/` — concurrent `cargo build` from different worktrees will **block each other** waiting for the lock
- (-) The shared target dir may cause subtle issues with different features or versions across worktrees
- (-) Need an absolute path (relative paths resolve differently in each worktree)

**Verdict: Shared target dir defeats the purpose of parallel execution** because cargo serializes builds through its file lock. Separate target dirs are correct for parallel execution.

### sccache and Build Caching

`sccache` caches compilation artifacts by input hash. Two worktrees compiling the same dependency with the same flags get a cache hit.

```bash
# Install
cargo install sccache

# Configure
export RUSTC_WRAPPER=sccache
# or in .cargo/config.toml:
# [build]
# rustc-wrapper = "sccache"
```

Trade-offs:
- (+) Cache hits for shared dependencies across worktrees
- (+) Transparent — doesn't change build semantics
- (+) Works with separate target dirs (each worktree builds independently, sccache deduplicates)
- (-) Adds a dependency on sccache being installed
- (-) First build of each worktree still takes time (cache is populated incrementally)
- (-) sccache itself has some overhead per compilation unit

**Verdict: sccache is the right answer.** It provides the caching benefits of a shared target dir without the lock contention. rloop should document (not enforce) sccache as a recommended configuration for parallel execution.

### Practical Parallelism Limits for Rust Projects

Empirical guidance:
- **2 parallel tasks** on a machine with 8-16 cores: each agent gets ~4-8 cores for compilation. Manageable.
- **3 parallel tasks** on a machine with 16+ cores: tight but workable with sccache.
- **4+ parallel tasks**: diminishing returns — CPU contention, memory pressure (8-12 GB per compilation), disk I/O bottleneck.

The real bottleneck isn't CPU but **context window cost**. Each parallel agent is an independent Claude session consuming tokens. The incremental cost of parallel execution is the sum of all agent costs, which is the same as sequential (each task costs the same tokens regardless of order). The benefit is wall-clock time reduction.

### CPU Limiting per Worktree

We could limit CPU usage per task:

```bash
# Limit cargo to N threads per worktree
CARGO_BUILD_JOBS=4 cargo build
```

This prevents one compilation from starving others. rloop could set `CARGO_BUILD_JOBS` proportionally:

```rust
let jobs_per_task = num_cpus::get() / config.max_parallel;
std::env::set_var("CARGO_BUILD_JOBS", jobs_per_task.to_string());
```

**Verdict: Worth considering but not critical for v1.** The agent controls when it runs `cargo build` and compilation jobs are typically bursty. Let the OS scheduler handle it initially.

### Configuration Recommendation

```toml
# .rloop/config.toml
[session]
max_parallel = 2

# Future:
# cargo_build_jobs = 4  # override CARGO_BUILD_JOBS per worktree
```

The default of 2 is safe for most machines. Users with powerful hardware can increase it. Document that sccache is recommended for parallel execution.

---

## 3. Daemon Mode Architecture

### What Daemon Mode Means

Instead of `rloop run 05` (run once, exit), a daemon runs continuously:
- Watches for new or changed tasks
- Automatically starts sessions for incomplete tasks
- Reports status to CLI queries
- Runs until stopped

### Approach A: Background Process with Unix Socket

rloop starts a background process that listens on a Unix domain socket for commands.

```bash
# Start daemon
rloop daemon start
# → forks to background, writes PID to .rloop/.daemon.pid
# → listens on .rloop/.daemon.sock

# Submit work
rloop run 05
# → if daemon running: sends "run 05" to socket, streams status back
# → if no daemon: runs inline (current behavior)

# Query status
rloop status
# → connects to socket, gets current state

# Watch a task
rloop watch 05
# → connects to socket, streams events for task 05

# Stop daemon
rloop daemon stop
# → sends shutdown signal via socket
```

**Implementation sketch:**

```rust
// daemon.rs
struct Daemon {
    socket: UnixListener,
    sessions: HashMap<TaskId, SessionHandle>,
    task_watcher: Option<FsWatcher>,
}

impl Daemon {
    async fn run(&mut self) {
        loop {
            tokio::select! {
                conn = self.socket.accept() => {
                    self.handle_client(conn).await;
                }
                task = self.task_watcher.next() => {
                    self.schedule_task(task).await;
                }
                _ = tokio::signal::ctrl_c() => {
                    self.shutdown().await;
                    break;
                }
            }
        }
    }
}
```

**Trade-offs:**
- (+) Clean separation of client (CLI) and server (daemon)
- (+) Unix sockets are fast, reliable, well-understood
- (+) Multiple CLI clients can connect simultaneously
- (+) Natural model for `rloop watch` (long-lived connection streaming events)
- (-) Process management complexity (PID files, socket cleanup, orphan detection)
- (-) Two modes of operation (inline vs. daemon) increases surface area
- (-) Unix sockets are platform-specific (though this is a non-issue for now)

### Approach B: Systemd Service / launchd Agent

Run rloop as a system service managed by systemd (Linux) or launchd (macOS).

```ini
# /etc/systemd/system/rloop.service
[Unit]
Description=rloop task executor

[Service]
Type=simple
ExecStart=/usr/local/bin/rloop daemon --repo /path/to/repo
Restart=on-failure
User=developer

[Install]
WantedBy=multi-user.target
```

```xml
<!-- ~/Library/LaunchAgents/com.rloop.daemon.plist -->
<dict>
    <key>Label</key><string>com.rloop.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/rloop</string>
        <string>daemon</string>
        <string>--repo</string>
        <string>/path/to/repo</string>
    </array>
    <key>KeepAlive</key><true/>
</dict>
```

**Trade-offs:**
- (+) Automatic restart on crash
- (+) System logging (journald / syslog)
- (+) Standard process management (systemctl start/stop/status)
- (-) Requires system configuration outside the repo
- (-) Platform-specific setup
- (-) Overkill for development use — systemd services are for production daemons

### Approach C: File-Based Status (No Socket)

The daemon runs as a background process but communicates entirely through the filesystem:
- `.rloop/.daemon.pid` — PID file
- `.rloop/.daemon.status.json` — current state (running tasks, completed, failed)
- `.rloop/sessions/` — event logs (already exist)

CLI commands read these files instead of connecting to a socket.

```bash
rloop status
# → reads .daemon.status.json, prints current state

rloop run 05
# → writes a request file to .rloop/.daemon/queue/05.request
# → daemon picks it up via filesystem watch
```

**Trade-offs:**
- (+) No socket complexity
- (+) Status is always available even after daemon crash (files persist)
- (+) Cross-platform (no Unix sockets)
- (-) No real-time streaming (rloop watch would need to poll the log file)
- (-) File-based IPC is clunky and race-prone
- (-) No way to send commands to the daemon (only queue files)

### Task Discovery

How does the daemon know when to start new tasks?

**Option 1: Filesystem watch.** Use `notify` crate to watch `.rloop/tasks/` for new or modified `.md` files.
- (+) Immediate reaction to changes
- (-) File watches can be unreliable (especially on network filesystems)
- (-) Doesn't distinguish "user is still editing" from "file is ready"

**Option 2: Polling.** Periodically scan `.rloop/tasks/` for incomplete tasks.
- (+) Simple, reliable
- (-) Latency between task creation and execution (poll interval)
- (-) Unnecessary I/O when nothing has changed

**Option 3: Explicit signal.** The daemon only starts tasks when explicitly told:
```bash
rloop run 05           # submit task 05 to daemon
rloop run --all        # submit all incomplete tasks
```
- (+) No surprises — daemon only runs what you tell it to
- (+) Simplest implementation
- (-) Doesn't feel like a "daemon" — more like a task queue server

**Option 4: Git-based.** Watch for new commits on a branch:
```bash
rloop daemon --watch main
# → polls `git log` on main, starts tasks when new task files appear
```
- (+) Natural for git-centric workflow
- (+) Task files are "ready" when committed
- (-) Couples daemon behavior to git operations

**Recommendation: Option 3 (explicit signal) for v1, with Option 1 (filesystem watch) as an optional enhancement.** The daemon should be a task queue server that runs what you tell it. Auto-discovery is a convenience that can be added later.

### Session Lifecycle in Daemon Mode

In the current model, a "session" is one invocation of rloop. In daemon mode, the daemon manages multiple sessions:

```
Daemon
├── Session A (rloop run 05) — running tasks 01, 02, 05
├── Session B (rloop run 10) — running task 10
└── Session C (rloop run --all) — running all incomplete tasks
```

Key questions:
- **Can multiple sessions run simultaneously?** Currently `SessionLock` prevents this. The daemon would own the lock and manage concurrency internally.
- **How do sessions share the repo?** Each session has its own session branch, and worktrees are isolated. Multiple sessions can coexist as long as they don't target overlapping tasks.
- **What about conflicting tasks?** If session A and session B both try to run task 05, the daemon should reject the second request.

**Architecture for daemon sessions:**

```rust
struct DaemonState {
    active_sessions: HashMap<SessionId, SessionHandle>,
    active_tasks: HashSet<TaskId>,  // prevents double-scheduling
    pending_requests: VecDeque<RunRequest>,
}
```

### Status Reporting

The daemon needs to report:
- Which sessions are running
- Which tasks are in-flight, completed, failed
- Per-task metrics (duration, cost so far)
- Overall daemon health

**`rloop status` output:**

```
rloop daemon — running since 2025-02-06 14:30:22
  Active sessions: 2
  Tasks in-flight: 03, 07
  Tasks completed: 01, 02, 05, 06
  Tasks failed: 04

Session rloop-05 (started 14:31:00)
  01 done  (2m 15s, $0.08)
  02 done  (4m 32s, $0.15)
  03 running (1m 45s so far)
  04 failed (3m 20s, $0.12) — "tests failed after 3 attempts"
  05 waiting (blocked by 03)
```

**`rloop watch 05`** streams events for a specific task:

```
→ task 05 waiting (dependency 03 in-flight)
→ task 05 starting (worktree created)
  CALL Read src/lib.rs
  CALL Edit src/lib.rs
  CALL Bash cargo test
✓ task 05 — implemented audio preprocessing
```

### Graceful Shutdown

```
Ctrl+C or `rloop daemon stop`:
1. Stop accepting new requests
2. Wait for in-flight tasks to complete (with timeout)
3. If timeout: interrupt Claude streams, cleanup worktrees
4. Remove .daemon.pid and .daemon.sock
5. Exit
```

The current Ctrl+C handling in `main.rs` already does worktree cleanup. The daemon version extends this to multiple concurrent sessions.

### Recommended Approach

**Approach A (background process with Unix socket) for daemon mode.**

Rationale:
- Unix sockets provide real-time bidirectional communication (needed for `rloop watch`)
- Process management is well-understood (PID file + signal handling)
- Natural evolution: the current `rloop run` becomes `rloop daemon run-inline` (the non-daemon path) and the socket path is the daemon path
- Maps to future cloud workers (socket becomes TCP/gRPC)

**But: daemon mode is not needed for v1.** Parallel DAG execution within a single `rloop run` invocation is the priority. Daemon mode is a v2 feature.

---

## 4. Future Cloud Workers (Hetzner)

### The Vision

Eventually, daemons run on remote Hetzner machines. A central coordinator (or the local daemon) dispatches tasks to remote workers. Each worker:
- Has its own copy of the repo
- Runs tasks in local worktrees
- Reports results back to the coordinator
- Pushes completed branches to a shared remote

### What This Means for Today's Architecture

We need to avoid architectural decisions now that would make cloud workers impossible later. Key areas:

### Task Storage: Files vs. Database vs. Hybrid

**Current: Files.** Tasks are `.rloop/tasks/*.md` in the repo, committed to git. This is simple and works for single-machine execution.

**For multi-machine:**
- Workers need access to task files. If tasks are in git, workers `git pull` to get them.
- Task state (completed, in-flight, failed) needs to be coordinated. Currently `completed: true` is written to the file on the session branch. With multiple machines, this creates merge conflicts.

**Options:**

**A. Stay git-centric.** Tasks in git, coordination via branches and locks.
- Coordinator assigns tasks to workers
- Worker pulls repo, creates worktree, executes, pushes branch
- Coordinator merges branches, updates task files
- (+) No new infrastructure
- (-) Git isn't a task queue — coordinating via branches is fragile
- (-) Push/pull latency between workers

**B. Task metadata in a database, content in git.** Task files stay in git (they're part of the project), but runtime state (assigned_to, status, started_at) lives in a lightweight database (SQLite for single-machine, PostgreSQL for multi-machine).
- (+) Clean separation of definition (git) and runtime state (DB)
- (+) Database provides atomic state transitions, queries, locking
- (+) Workers read task content from git, state from DB
- (-) New dependency (database)
- (-) State can diverge from git (task file says completed: false, DB says completed: true)

**C. API-centric.** Tasks are managed entirely through an API (REST/gRPC). The API stores tasks in a database. Git is used only for code, not task definitions.
- (+) Clean API boundary — workers are just API clients
- (+) No git-based coordination complexity
- (-) Moves task definitions out of the repo (loses git history, review, co-location)
- (-) Requires running an API server

**D. Hybrid: files define, API coordinates.** Task files in git define the work. An API server (or lightweight protocol) coordinates execution state. Workers `git pull` for task content, query the API for assignment.
- (+) Best of both worlds — tasks are code-reviewable in git, execution is API-coordinated
- (+) Natural evolution from the current file-based system
- (-) Two sources of truth (but clearly separated: git = definition, API = runtime)

**Recommendation: Option A for now, design for Option D.** Keep tasks in git. Don't add a database yet. But structure the code so that "load task" and "update task state" are separate operations that could be backed by different storage in the future.

The key abstraction:
```rust
trait TaskStore {
    fn load_task(&self, id: TaskId) -> Result<Task>;
    fn load_all_tasks(&self) -> Result<Vec<Task>>;
    fn mark_completed(&self, id: TaskId) -> Result<()>;
    fn mark_in_progress(&self, id: TaskId, worker: &str) -> Result<()>;
}

// Current implementation
struct FileTaskStore { tasks_dir: PathBuf }

// Future implementation
struct ApiTaskStore { endpoint: Url, auth_token: String }
```

### Session Branches with Multiple Machines

The current model: one session branch per `rloop run`, one machine. With multiple machines:

**Option 1: Shared session branch.** All workers push to the same session branch. Requires coordination to avoid conflicts.
- Fragile. Git doesn't support concurrent pushes to the same branch.

**Option 2: Per-worker branches.** Each worker pushes to `rloop/{task_id}/{worker_id}`. The coordinator merges them into the session branch.
- Clean. Each worker owns its branch. Coordinator handles merges.

**Option 3: Per-task branches, no session branch.** Each task gets its own branch: `rloop-task-05`. The coordinator (or human) merges them into the main branch in dependency order.
- Simplest for workers. But loses the "session" concept — a coherent set of changes reviewed as a unit.

**Recommendation: Option 2.** Per-worker branches pushed to a shared remote. The coordinator (which could be the local daemon or a dedicated service) handles merging into the session branch. This maps cleanly to the current per-worktree branching model.

### Abstractions to Build Now

To keep cloud workers feasible without building them:

1. **`StepExecutor` trait (already exists).** The trait in `step.rs` already abstracts step execution. A remote worker would implement `StepExecutor` as an RPC call instead of a local Claude session.

2. **Event sourcing (already exists).** The JSONL session log is already an event stream. Remote workers would send events over the network instead of writing to a local file.

3. **TaskStore trait (add later).** Extract task loading into a trait when needed. For now, the functions in `task.rs` are fine.

4. **Merge serialization (build for parallel).** The merge-lock pattern needed for parallel local execution is the same pattern needed for merging remote worker results. Build it once for parallel, reuse for cloud.

### What NOT to Build Now

- Don't add a database.
- Don't add network protocols (gRPC, REST).
- Don't add remote worker registration/discovery.
- Don't add distributed locking (etcd, Redis).
- Don't abstract away git — the git-centric model works and is a feature, not a limitation.

---

## 5. Build Order: Parallel First, Then Daemon

### Why Parallel Before Daemon

1. **Parallel is a smaller delta.** It changes `execute_dag` from sequential to concurrent. The rest of the system (task loading, worktree management, logging, CLI) stays the same.

2. **Parallel solves a real problem now.** A DAG with 10 tasks where 5 are independent takes half the time with `max_parallel=2`. Users get immediate value.

3. **Daemon requires parallel.** The daemon runs multiple tasks concurrently by definition. Building parallel first means the daemon can reuse the parallel execution infrastructure.

4. **Daemon requires more design.** Socket protocol, process management, status reporting, session lifecycle — these are all new subsystems. Parallel just extends the existing `execute_dag` function.

### Implementation Phases

**Phase 1: Parallel DAG execution (in-process)**
- Add `max_parallel` to `Config`
- Extract frontier detection from topological sort
- Replace `for` loop with `FuturesUnordered`-based scheduler
- Add merge serialization (rebase + fast-forward with lock)
- Add channel-based logger
- Update tests

**Phase 2: Daemon mode**
- Add `rloop daemon start/stop`
- Unix socket server
- `rloop status` and `rloop watch`
- Multi-session management
- Task queue with conflict detection
- Graceful shutdown

**Phase 3: Cloud workers (if/when needed)**
- `TaskStore` trait extraction
- Remote step executor
- Network event transport
- Worker registration and health checking

---

## 6. Trade-off Summary

| Feature | Approach | Complexity | Value | Risk |
|---------|----------|------------|-------|------|
| Parallel execution | FuturesUnordered | Medium | High (2x speedup for independent tasks) | Low (fallback to sequential) |
| Merge strategy | Rebase + fast-forward | Medium | High (clean history) | Low (fallback to merge commit) |
| Logger for parallel | Channel-based | Low | Medium (correct event ordering) | Low |
| Concurrency cap | Configurable, default 2 | Low | Medium (safety valve) | Low |
| Compilation caching | Document sccache | Low | Medium (faster builds) | None |
| Daemon mode | Unix socket server | High | Medium (convenience) | Medium (process management) |
| Cloud workers | Per-worker branches | High | High (horizontal scaling) | High (distributed systems) |
| Task storage | Files (current) | None | N/A (already built) | None |
| Task storage | Trait abstraction | Low | Low (future-proofing) | Low |

---

## 7. Open Questions

### Must Resolve Before Building Parallel

1. **Merge strategy: rebase or merge commit?** Rebase gives linear history but can fail on conflicts. Merge commits always work but create noisy history. Recommendation: rebase with merge-commit fallback. Is this acceptable?

2. **What happens to the current `fast_forward_ref` approach?** It only works for sequential execution. For parallel, we need git merge or rebase. Should we keep `fast_forward_ref` for the `max_parallel=1` case (backwards compatibility) or always use the parallel-safe merge path?

3. **Should `max_parallel` default to 1 or 2?** Defaulting to 1 preserves current behavior but nobody benefits from parallelism until they change the config. Defaulting to 2 gives immediate value but changes behavior. Recommendation: default to 1, document the parallel option.

4. **How does Ctrl+C work with parallel tasks?** Currently it's handled in `main.rs` with a `tokio::select`. With parallel tasks, cancellation needs to interrupt all in-flight agents and clean up all worktrees. The `cleanup_worktrees` function already handles this, but the in-flight Claude streams need to be interrupted too.

### Must Resolve Before Building Daemon

5. **Is daemon mode worth the complexity?** The primary use case is "submit tasks and check back later." This can also be achieved with `rloop run --all &` (background job) plus reading the session log. Daemon mode adds: task queue, status API, `rloop watch`, auto-restart. Is this worth the engineering cost?

6. **Should the daemon manage multiple repos?** Or is it one daemon per repo? Recommendation: one per repo, matching the current `SessionLock` model. Multi-repo is a separate concern.

7. **How does the daemon interact with `SessionLock`?** The daemon effectively holds the lock for its entire lifetime. Does this mean `rloop run 05` (without daemon) fails if the daemon is running? Recommendation: if daemon is running, `rloop run` forwards to the daemon via socket. If no daemon, runs inline (current behavior).

### Worth Discussing

8. **Should parallel execution be opt-in or automatic?** If the DAG has independent tasks and `max_parallel > 1`, should rloop automatically parallelize? Or require an explicit flag like `--parallel`? Recommendation: automatic based on config. If `max_parallel > 1`, parallelize automatically. No flag needed.

9. **Should we track which tasks are "safe to parallelize"?** Some tasks might modify shared files even though they're not in each other's dependency chains. A `parallel_safe: false` frontmatter field could disable parallelism for specific tasks. Is this needed? Recommendation: not for v1. If tasks conflict, the merge/rebase step will catch it.

10. **What's the right abstraction boundary for future cloud workers?** Should `StepExecutor` become `TaskExecutor` that handles the full task lifecycle (worktree create, step, merge, cleanup)? Or should the remote worker only handle the step, with the coordinator managing git operations? Recommendation: remote worker handles the step only. Coordinator manages git. This keeps git operations centralized and avoids distributed merge conflicts.
