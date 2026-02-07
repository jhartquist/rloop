# Research: Human Queue (HumanQ)

## Context

Today rloop is a non-interactive loop: agents run, verification passes or fails, tasks succeed or fail. The human's role is before (write task specs) and after (review session branches). There is no mechanism for agents to communicate with humans during execution.

The Human Queue (HumanQ) changes this. Agents get tools to ask questions, log assumptions, and propose new tasks. Humans process these items asynchronously — one at a time, in a batch review session — without ever blocking the agent loop. When an agent needs a human answer, it can either continue without one (non-blocking) or yield the step for later resumption (blocking).

This is the core differentiating feature of rloop. The key insight from the VISION document: "Humans don't block the loop. Agents work continuously." And: "Humans review asynchronously: batch process queue items, zero wait latency between items, answers unblock tasks for future steps."

---

## 1. Agent-Side Tools and Blocking Semantics

### New MCP Tools

Three new tools for the agent, registered in the `rloop` MCP server alongside `complete` and `context_usage`:

#### `ask_question`

```json
{
  "name": "ask_question",
  "description": "Ask the human a question. Use when you need clarification, a decision, or approval before proceeding.",
  "parameters": {
    "question": { "type": "string", "description": "The question to ask" },
    "context": { "type": "string", "description": "Background context that helps the human answer" },
    "blocking": { "type": "boolean", "description": "If true, this step will pause until the human answers. If false, the question is queued and you should continue working with your best assumption." }
  }
}
```

#### `log_assumption`

```json
{
  "name": "log_assumption",
  "description": "Record an assumption you're making. The human can review and correct it later.",
  "parameters": {
    "assumption": { "type": "string", "description": "What you're assuming" },
    "reasoning": { "type": "string", "description": "Why you're making this assumption" },
    "alternatives": { "type": "string", "description": "Alternative approaches you considered" }
  }
}
```

#### `propose_task`

```json
{
  "name": "propose_task",
  "description": "Propose a new task you've discovered while working. The human can approve, reject, or edit it.",
  "parameters": {
    "title": { "type": "string", "description": "Short title for the proposed task" },
    "description": { "type": "string", "description": "Full description in task markdown format" },
    "depends_on": { "type": "array", "items": { "type": "string" }, "description": "Task IDs this would depend on" },
    "reasoning": { "type": "string", "description": "Why this task is needed" }
  }
}
```

### Blocking vs Non-Blocking Semantics

The fundamental question: what happens when an agent calls `ask_question` with `blocking: true`?

#### Approach A: True Blocking with Step Suspension

The step pauses execution. The agent's Claude session is interrupted. The worktree and conversation state are preserved. When the human answers, the step resumes — either by rebuilding context from the session log or by resuming the Claude Code session.

**How it works in the code:**

```rust
// In the ask_question tool closure:
if blocking {
    // 1. Write question to queue
    queue.enqueue(QueueItem::Question { ... })?;

    // 2. Signal the step executor to suspend
    suspend_ref.store(true, Ordering::SeqCst);

    // 3. Return a message telling the agent we're pausing
    return Ok(tool_ok("Question queued. This step will now pause until the human answers."));
}
```

```rust
// In execute_step, after stream processing:
if data.suspended {
    return Ok(StepResult {
        task_id: task.id,
        outcome: StepOutcome::Blocked {
            reason: "waiting for human answer".to_string(),
            queue_item_id: item_id,
        },
        // ... metrics ...
    });
}
```

This requires a new `StepOutcome` variant:

```rust
pub enum StepOutcome {
    Success { summary: String },
    Failed { reason: String, learnings: String },
    Blocked { reason: String, queue_item_id: String },  // NEW
}
```

And the session's `execute_task` must handle `Blocked`:

```rust
match &step_result.outcome {
    StepOutcome::Success { summary } => { /* merge, continue */ }
    StepOutcome::Failed { .. } => { /* retry or mark failed */ }
    StepOutcome::Blocked { .. } => {
        // Don't cleanup worktree — it needs to persist
        // Don't mark failed — it's not failed, just waiting
        // Return a TaskOutcome::Blocked so the session can handle it
        return Ok(TaskOutcome::Blocked(task_metrics));
    }
}
```

**Trade-offs:**
- (+) Clean semantics — the agent truly pauses and waits for the answer
- (+) The answer is guaranteed to be available when the step resumes
- (+) No wasted tokens continuing on a potentially wrong assumption
- (-) Requires session resume capability (complex)
- (-) Worktree must persist while blocked (can't clean up)
- (-) Claude Code session state must be checkpointed or rebuilt
- (-) Blocked tasks block their dependents in the DAG

**What happens to the worktree while blocked?**
The worktree stays locked. It contains the agent's in-progress work. When the human answers, a new step attempt runs in the same worktree with the answer included in `prior_learnings` (or a dedicated `human_answers` field). The agent gets the original task description + "Human answer to your question: ..." and continues from where the previous agent left off.

The worktree must NOT be cleaned up on suspension. This means `cleanup_worktree` is only called on success or final failure, never on `Blocked`.

#### Approach B: Fake Blocking via Retry

The agent continues working. When it calls `ask_question(blocking=true)`, the tool returns immediately with "question queued, step will end." The step then completes with a special outcome (`StepOutcome::NeedsHumanInput`). The task is marked as pending human input. When the human answers, the task is re-run from scratch, with the answer injected into the prompt.

```rust
// In ask_question tool closure:
if blocking {
    queue.enqueue(QueueItem::Question { ... })?;
    *summary_ref.lock().unwrap() = None;  // don't count as success
    stop_ref.store(true, Ordering::SeqCst);  // interrupt stream

    return Ok(tool_ok(
        "Question queued. This step will end. The task will restart when the human answers."
    ));
}
```

**Trade-offs:**
- (+) No session resume — just re-run the task fresh
- (+) Simpler implementation — uses existing retry infrastructure
- (+) Worktree is cleaned up normally (no dangling worktrees)
- (+) Fresh context window on retry (no accumulated garbage)
- (-) Wastes tokens — all work before the question is re-done on retry
- (-) The agent might ask the same question again if the prompt isn't carefully constructed
- (-) Doesn't truly "block" — it's a failure with special handling
- (-) The re-run might take a different path and not even need the answer

#### Approach C: Non-Blocking Only (V1)

All tools are non-blocking. `ask_question` queues the question and immediately returns "question queued." The agent is instructed to continue with its best assumption. If the human answers before the step completes, the answer could theoretically be injected (but this is complex). More realistically, the answer is available for the next run of the task.

```rust
// ask_question always returns immediately:
queue.enqueue(QueueItem::Question { ... })?;
Ok(tool_ok(format!(
    "Question queued for human review. Continue with your best judgment. \
     If the task is re-run later, the human's answer will be available."
)))
```

`log_assumption` is inherently non-blocking. `propose_task` is inherently non-blocking.

**Trade-offs:**
- (+) Simplest implementation — no resume, no blocking, no special outcomes
- (+) Agents always make progress (never stuck waiting)
- (+) Works with the current session model unchanged
- (+) Natural fit for the VISION doc's "agents work continuously" philosophy
- (-) Human answers arrive too late for the current run
- (-) Questions are advisory only — agents must work without answers
- (-) Less useful for truly blocking decisions ("should I use library A or B?")
- (-) The human reviews a queue of items from tasks that have already completed or failed

### Recommended Approach

**Approach C (non-blocking only) for v1, designed to support Approach B (fake blocking via retry) in v1.1, with Approach A (true blocking) as a v2 feature tied to daemon mode.**

Rationale:
1. Non-blocking tools are immediately useful. `log_assumption` and `propose_task` don't need blocking. `ask_question` is valuable even non-blocking — the human can answer before the next run.
2. Fake blocking via retry is a natural extension: when `ask_question(blocking=true)` is called, end the step with a special outcome, and on the next run, inject the answer. This requires no session resume.
3. True blocking requires daemon mode (something must be running to detect when the answer arrives and restart the step). Without a daemon, there's nobody to resume the blocked task.

### Interaction with the DAG

When a task is `Blocked`:
- **Non-blocking questions:** The task continues to completion. DAG is unaffected.
- **Blocking questions (v1.1):** The step ends with `NeedsHumanInput`. The task is NOT marked as failed. Downstream dependents are skipped (same as failed). When the session is re-run after the human answers, the task re-executes with the answer available.
- **Daemon mode (v2):** Blocked tasks are re-queued automatically when the answer arrives. The daemon monitors the queue and triggers re-execution.

---

## 2. Queue Storage and State Machine

### Queue Item Schema

```rust
pub struct QueueItem {
    pub id: String,                    // UUID or timestamp-based
    pub item_type: QueueItemType,
    pub task_id: TaskId,
    pub session_id: SessionId,
    pub status: QueueItemStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub response: Option<QueueResponse>,
}

pub enum QueueItemType {
    Question {
        question: String,
        context: String,
        blocking: bool,
    },
    Assumption {
        assumption: String,
        reasoning: String,
        alternatives: String,
    },
    ProposedTask {
        title: String,
        description: String,
        depends_on: Vec<String>,
        reasoning: String,
    },
}

pub enum QueueItemStatus {
    Pending,       // waiting for human
    InReview,      // human is looking at it (optional, for TUI tracking)
    Answered,      // human has responded
    Processed,     // response has been consumed by a step
    Expired,       // task completed/failed without needing the answer
}

pub struct QueueResponse {
    pub content: String,
    pub responded_at: DateTime<Utc>,
    // For proposed tasks:
    pub approved: Option<bool>,
    // For assumptions:
    pub correction: Option<String>,
}
```

### State Machine

```
                    ┌──────────────────────┐
                    │                      │
                    ▼                      │
Pending ──────► InReview ──────► Answered ──────► Processed
   │                │                               │
   │                │                               │
   └────────────────┴───────────► Expired ◄─────────┘
                                     ▲
                                     │
                          (task completed/failed/
                           session ended without
                           consuming the answer)
```

Transitions:
- **Pending → InReview**: Human opens the review interface and starts reviewing this item
- **InReview → Answered**: Human submits a response
- **Answered → Processed**: A step execution reads the answer and uses it
- **Pending → Expired**: Task completed or failed without the answer being needed (the agent continued with an assumption)
- **InReview → Expired**: Same as above, but while the human was reviewing
- **Answered → Expired**: The answer arrived but the task already succeeded without it

The `Expired` transition is important for queue hygiene. Without it, old questions from successful tasks would pile up.

### Approach A: File-Based Queue

Each queue item is a file in `.rloop/queue/`:

```
.rloop/
├── queue/
│   ├── 20260207-143022-abc123.json   # individual item
│   ├── 20260207-143025-def456.json
│   └── 20260207-143030-ghi789.json
```

Each file is a JSON-serialized `QueueItem`.

**Reading the queue:** Glob `.rloop/queue/*.json`, parse each, sort by `created_at`.

**Updating status:** Read file, update status, write back. Use `write_with_backup` from `log.rs` for atomicity.

**Trade-offs:**
- (+) Simple, no dependencies
- (+) Inspectable — `cat .rloop/queue/*.json | jq`
- (+) Survives crashes — files persist
- (+) Works with git (queue can be committed for cross-machine sharing, though that's future)
- (+) Consistent with rloop's file-centric design (tasks are files, sessions are JSONL files)
- (-) Listing items requires reading every file (O(n) I/O)
- (-) No atomic multi-item updates (e.g., expire all items for a task)
- (-) Possible race conditions if daemon mode has concurrent readers/writers
- (-) Files accumulate over time (needs cleanup strategy)

**File naming convention:** `{timestamp}-{short_uuid}.json` for natural sort order.

### Approach B: SQLite Database

```sql
CREATE TABLE queue_items (
    id TEXT PRIMARY KEY,
    item_type TEXT NOT NULL,      -- 'question', 'assumption', 'proposed_task'
    task_id INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    content TEXT NOT NULL,        -- JSON blob with type-specific fields
    response TEXT,                -- JSON blob with response
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_status ON queue_items(status);
CREATE INDEX idx_task_id ON queue_items(task_id);
```

Location: `.rloop/queue.db`

**Trade-offs:**
- (+) Efficient queries (status filtering, task lookup)
- (+) Atomic transactions (expire all items for a task in one operation)
- (+) Single file, no directory sprawl
- (+) Handles concurrency via SQLite's built-in locking
- (+) Natural fit for daemon mode (multiple readers, serialized writes)
- (-) New dependency (`rusqlite` crate)
- (-) Not inspectable without tooling (`sqlite3` CLI or a custom `rloop queue list`)
- (-) Opaque binary format (can't use git diff, can't manually edit)
- (-) Requires migration strategy for schema changes
- (-) Doesn't match rloop's file-centric philosophy

### Approach C: Single JSONL File

All queue items appended to `.rloop/queue.jsonl`, similar to session logs.

```
{"id":"abc","type":"question","status":"pending",...}
{"id":"abc","type":"status_change","status":"answered","response":"..."}
{"id":"def","type":"assumption","status":"pending",...}
```

This is an event-sourcing approach — the current state is derived by replaying the log.

**Trade-offs:**
- (+) Append-only (crash-safe, no partial writes)
- (+) Consistent with session log format (JSONL everywhere)
- (+) Simple implementation
- (-) Requires full scan to get current state (no index)
- (-) File grows without bound (needs compaction)
- (-) Status updates are new lines, not modifications (harder to reason about)

### Approach D: Hybrid (Files + Index)

Individual JSON files for items (like Approach A), plus a lightweight index file (`.rloop/queue/index.json`) that caches status and ordering:

```json
{
  "items": [
    { "id": "abc123", "status": "pending", "task_id": 3, "type": "question", "created_at": "..." },
    { "id": "def456", "status": "answered", "task_id": 5, "type": "assumption", "created_at": "..." }
  ]
}
```

The index is rebuilt from files on startup (source of truth is individual files) and kept in sync during the session.

**Trade-offs:**
- (+) Fast listing (read index only)
- (+) Individual files remain inspectable
- (+) Index is a cache, not the source of truth (recoverable)
- (-) More complexity (keep index in sync)
- (-) Two representations to maintain

### Recommended Storage

**Approach A (file-based) for v1.** Simple, inspectable, consistent with rloop's design. The O(n) listing overhead is negligible for expected queue sizes (dozens of items, not thousands). Transition to SQLite only if/when daemon mode requires concurrent access patterns that files can't handle.

Add `.rloop/queue/` to `.gitignore` initially. The queue is ephemeral — items expire as tasks complete. If cross-machine queue sharing is needed (cloud workers), the storage can be swapped to a shared database later.

### Handling Stale Items

When a task completes or fails, all its pending queue items should be marked `Expired`:

```rust
pub fn expire_items_for_task(queue_dir: &Path, task_id: TaskId) -> anyhow::Result<u32> {
    let mut expired_count = 0;
    for entry in std::fs::read_dir(queue_dir)? {
        let path = entry?.path();
        if path.extension() != Some("json".as_ref()) { continue; }
        let item: QueueItem = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        if item.task_id == task_id && matches!(item.status, QueueItemStatus::Pending | QueueItemStatus::InReview) {
            let mut updated = item;
            updated.status = QueueItemStatus::Expired;
            updated.updated_at = Utc::now();
            write_with_backup(&path, serde_json::to_string_pretty(&updated)?.as_bytes())?;
            expired_count += 1;
        }
    }
    Ok(expired_count)
}
```

This is called from `finish_successful_task` and `finish_failed_task` in `session.rs`.

When a task is retried (within a session), pending questions from previous attempts should be checked for answers before being expired. If a human answered a question between retry attempts, the answer should be available to the new attempt.

### Queue Cleanup

Over time, expired and processed items accumulate. Options:
- **Auto-cleanup:** Delete expired/processed items older than N days (configurable, default 7)
- **Manual cleanup:** `rloop queue clean` command
- **On session start:** Clean items from previous sessions that are expired/processed

Recommendation: auto-cleanup on session start. Delete expired/processed items older than 7 days. This keeps the queue directory manageable without requiring manual intervention.

---

## 3. Human-Side Interface and Workflow

### The Key UX Insight

Unlike Claude Code's `AskUserQuestion` which pauses execution, presents all pending items, and blocks until the human responds to all of them, HumanQ should work differently:

1. Show one item at a time
2. Human answers/acknowledges it
3. Immediately dispatch the answer for processing (LLM workflow, task creation, etc.)
4. Show the next item while the previous answer is being processed
5. Zero wait time between items

This is a batch review session with streaming dispatch. The human's time is precious — don't make them wait for the system between items.

### Approach A: TUI with Ratatui

A full terminal UI built with [ratatui](https://ratatui.rs/) for reviewing queue items.

**Layout:**

```
┌─ rloop queue ──────────────────────────────────────────────────────┐
│                                                                     │
│  ┌─ Queue (3 items) ──────────────────────────────────────────────┐ │
│  │  ▶ [?] Task 05: Should I use `reqwest` or `ureq` for HTTP?    │ │
│  │    [!] Task 03: Assuming UTF-8 encoding for all input files    │ │
│  │    [+] Task 07: Proposed: Add retry logic for API calls        │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                     │
│  ┌─ Question from Task 05 ────────────────────────────────────────┐ │
│  │                                                                 │ │
│  │  Should I use `reqwest` or `ureq` for HTTP requests?           │ │
│  │                                                                 │ │
│  │  Context: The task requires making HTTP requests to an API.    │ │
│  │  `reqwest` is async and feature-rich. `ureq` is synchronous   │ │
│  │  and simpler. The project already uses tokio.                  │ │
│  │                                                                 │ │
│  │  Blocking: yes (step will resume when answered)                │ │
│  │                                                                 │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                                                                     │
│  Answer: █                                                          │
│                                                                     │
│  [Enter] Submit  [s] Skip  [Esc] Quit                              │
└─────────────────────────────────────────────────────────────────────┘
```

**Trade-offs:**
- (+) Rich, interactive experience
- (+) Visual context (see the queue, the current item, processing status)
- (+) Multi-line input editing for complex answers
- (+) Can show processing status (answer being dispatched to LLM) in real time
- (+) Professional look — this is the "product" experience for rloop
- (-) Significant development effort (ratatui is low-level)
- (-) Adds dependency on ratatui (and crossterm/termion for input handling)
- (-) Testing TUI code is harder than testing CLI code
- (-) Overkill for v1 if the queue has 3 items

### Approach B: Sequential CLI Prompts

Simple `println!`/`stdin` loop. Show items one at a time, collect input, move to next.

```
$ rloop queue review

Queue: 3 items pending

[1/3] QUESTION from task 05 (blocking)
  Should I use `reqwest` or `ureq` for HTTP requests?

  Context: The task requires making HTTP requests to an API.
  `reqwest` is async and feature-rich. `ureq` is synchronous
  and simpler. The project already uses tokio.

  Answer (or 's' to skip): Use reqwest since we're already async with tokio
  ✓ Answer recorded, processing...

[2/3] ASSUMPTION from task 03
  Assuming UTF-8 encoding for all input files

  Reasoning: No encoding specification in the task description.
  Standard practice for modern Rust projects.

  [a]ck, [c]orrect: a
  ✓ Acknowledged

[3/3] PROPOSED TASK from task 07
  Title: Add retry logic for API calls
  Description: The API occasionally returns 429 (rate limited)...

  [a]pprove, [r]eject, [e]dit: a
  ✓ Approved, creating task...

Queue review complete: 2 answered, 1 approved, 0 skipped
```

**Trade-offs:**
- (+) Minimal implementation (just print and readline)
- (+) Familiar UX (like git interactive rebase prompts)
- (+) Works everywhere (no TUI dependencies)
- (+) Easy to test
- (-) Less visual context (can't see the full queue at a glance)
- (-) No multi-line input without a workaround (open $EDITOR?)
- (-) Can't show async processing status while asking next question
- (-) Feels basic compared to the TUI vision

### Approach C: Editor-Based Review

Write queue items to a temporary file, open in `$EDITOR`, parse the result.

```markdown
# rloop Queue Review

## [QUESTION] Task 05 — HTTP client library (blocking)

Should I use `reqwest` or `ureq` for HTTP requests?

Context: The task requires making HTTP requests to an API...

> ANSWER: Use reqwest since we're already async with tokio

---

## [ASSUMPTION] Task 03 — UTF-8 encoding

Assuming UTF-8 encoding for all input files.

> ACK

---

## [PROPOSED TASK] Task 07 — Add retry logic for API calls

Title: Add retry logic for API calls
Description: The API occasionally returns 429...

> APPROVE
```

**Trade-offs:**
- (+) Full editing power (vim/emacs/vscode)
- (+) Multi-line answers are natural
- (+) Can review and edit everything before submitting
- (+) No TUI dependency
- (-) Batch-only (can't dispatch answers one at a time)
- (-) Parsing the edited file is fragile
- (-) Unintuitive for non-developers
- (-) Can't show real-time processing status

### Approach D: Web UI (Future)

A local web server serving a review dashboard. Items appear as cards, answers are typed in text areas, submitted via form.

**Trade-offs:**
- (+) Best UX — rich formatting, multi-line input, real-time updates
- (+) Accessible from any browser (including mobile for remote review)
- (+) Can integrate with notification systems
- (-) Heavy dependency (web framework, frontend)
- (-) Far more code than any terminal-based option
- (-) Not aligned with rloop's terminal-native philosophy for v1

### Recommended Approach

**Approach B (CLI prompts) for v1, with Approach A (TUI) as a v1.1 or v2 upgrade.**

The CLI prompt approach is:
1. Buildable in a day
2. Sufficient for early users (queue sizes will be small)
3. A proving ground for the queue review workflow before investing in TUI polish

The TUI is the target experience, but building it before the queue workflow is proven is premature.

### The Review Command

```bash
# Review all pending items
rloop queue review

# Review items for a specific task
rloop queue review --task 05

# List queue items without reviewing
rloop queue list

# Show queue stats
rloop queue stats
```

### For Each Item Type

**Questions:**
- Show question + context
- Human types an answer
- Answer is saved to the queue item file
- If blocking: task can be re-run to pick up the answer
- If non-blocking: answer is informational (available on re-run)

**Assumptions:**
- Show assumption + reasoning + alternatives
- Human acknowledges (it's fine) or corrects (provides the correct approach)
- Corrections are saved as learnings for future runs
- Corrections could trigger a task re-run if the assumption was wrong

**Proposed Tasks:**
- Show title + description + dependencies + reasoning
- Human approves, rejects, or edits
- Approved: create the task file in `.rloop/tasks/`, assign next ID
- Rejected: mark as expired with rejection reason
- Edit: open in `$EDITOR`, then approve the edited version

---

## 4. Answer-Processing LLM Workflow

### Why Process Answers with an LLM?

When a human answers a question, the raw answer text might need processing:

1. **Follow-up questions:** The answer raises new questions. "Use reqwest" → LLM generates "Should we use reqwest's async or blocking API?"
2. **Task creation:** The answer implies new work. "Add error handling for all API calls" → LLM creates a proposed task.
3. **Assumption validation:** When a human corrects an assumption, the LLM can identify which parts of the agent's work might be affected.
4. **Answer formatting:** Transform a terse human answer into structured context for the agent's next run.

### The Processing Pipeline

```
Human answers question
    ↓
Save raw answer to queue item
    ↓
Run LLM query (answer processing)
    ↓
LLM output: {
    formatted_answer: "...",      // cleaned up for agent consumption
    follow_up_questions: [...],   // new queue items to create
    proposed_tasks: [...],        // new tasks to propose
    affected_assumptions: [...]   // assumptions that might be wrong
}
    ↓
Create follow-up items / tasks
    ↓
Mark original item as Processed
```

### Using `run_query` for Answer Processing

This is the fourth consumer of the ancillary agent pattern from research 01:

```rust
let prompt = build_answer_processing_prompt(&question, &answer, &task_description);
let result = run_query(&prompt, &QueryConfig {
    model: Model::Sonnet,      // Sonnet is sufficient for text processing
    cwd: repo_root,
    tools: vec![],             // no tools needed
    setting_sources: vec![],
    output_format: Some(answer_processing_schema),
}).await?;
```

### The Processing Prompt

```
You are processing a human's answer to a question asked by a coding agent.

## Original Question
{question}

## Context
{context}

## Task Description
{task_description}

## Human's Answer
{answer}

## Instructions

Analyze the human's answer and produce:
1. A formatted version of the answer suitable for injection into an agent's prompt
   (clear, specific, actionable)
2. Any follow-up questions that the answer raises (if any)
3. Any new tasks implied by the answer (if any)

Respond with JSON:
{
  "formatted_answer": "...",
  "follow_up_questions": [
    { "question": "...", "context": "...", "blocking": false }
  ],
  "proposed_tasks": [
    { "title": "...", "description": "...", "depends_on": [] }
  ]
}
```

### When to Process Answers

**Option 1: Immediately during review (recommended).** After the human answers each item, the LLM processes it while the human reviews the next item. This is the "zero wait latency between items" principle.

```
Human answers item 1  →  LLM processes answer 1 (background)
Human answers item 2  →  LLM processes answer 2 (background)
Human answers item 3  →  LLM processes answer 3 (background)
...
All items reviewed     →  Wait for any remaining background processing
                       →  Show summary of follow-ups and new tasks
```

Implementation: spawn a tokio task for each answer's LLM processing. The review loop continues immediately.

**Option 2: Batch after review.** Collect all answers, then process them all at once after the review session ends.

Trade-off: Simpler implementation but the human waits at the end for processing. Defeats the "zero wait" principle.

**Option 3: No LLM processing for v1.** Just save the raw answer. The agent gets it verbatim on the next run. Follow-up questions and task proposals are handled manually.

Trade-off: Simplest. Fine for v1 since the primary value is the queue itself, not the post-processing.

### Recommended Approach

**Option 3 (no LLM processing) for v1.** Just save raw answers. The complexity of answer processing is not justified until the queue workflow is proven.

**Option 1 (immediate processing) for v1.1.** Once the queue is working and `run_query` is implemented (from research 01), add answer processing as a pipeline step. The key requirement is that processing happens in the background while the human continues reviewing.

### How Answers Reach the Agent

When a task is re-run (manually or by daemon), the step executor checks for answered queue items:

```rust
fn build_prompt(task: &Task, prior_learnings: &[String], queue_answers: &[QueueAnswer]) -> String {
    let mut prompt = /* ... existing prompt building ... */;

    if !queue_answers.is_empty() {
        prompt.push_str("\n\n## Human Answers\n");
        prompt.push_str("The following questions were asked by a previous attempt and answered by a human:\n\n");
        for qa in queue_answers {
            prompt.push_str(&format!("**Q:** {}\n**A:** {}\n\n", qa.question, qa.answer));
        }
    }

    prompt
}
```

The session's `execute_task` loads answered questions for the task before running the step:

```rust
let queue_answers = load_answered_questions(&queue_dir, task.id)?;
let prompt = build_prompt(task, &prior_learnings, &queue_answers);
```

---

## 5. Session Resume and Step Continuation

### The Resume Problem

If a step blocks on a question, the session needs to be resumable. This is the hardest part of the Human Queue design.

### What Needs to be Checkpointed

To resume a blocked step, we need:
1. **Worktree state:** The agent's in-progress file changes (already on disk in the worktree)
2. **Conversation history:** What the agent has done so far (in the session log as events)
3. **Prior learnings:** From any previous retry attempts (in memory during the session)
4. **Queue context:** Which question we're waiting on and what the answer is
5. **Session state:** Which tasks are completed, failed, in-progress (in the session log)

### Approach A: Claude Code Session Resume

The Claude Agent SDK supports session resume via session IDs. If we capture the `cc_session_id` from the System message (already done in `step.rs`), we could theoretically resume the Claude session.

```rust
let options = ClaudeAgentOptions::builder()
    .session_id(blocked_session_id)  // resume this session
    .model(model_string)
    .cwd(worktree_path)
    // ... same config ...
    .build();
```

The resumed session would get the human's answer injected as a new user message.

**Trade-offs:**
- (+) True continuation — the agent picks up exactly where it left off
- (+) No wasted tokens rebuilding context
- (+) Claude's internal conversation state is preserved
- (-) Unclear if the SDK supports this (needs investigation)
- (-) The session may have expired (Claude sessions have a TTL)
- (-) The agent's context window includes everything from before the block — may be at capacity
- (-) Tightly coupled to Claude Code's session management

### Approach B: Rebuild Context from Logs

Start a fresh Claude session. Reconstruct the context by:
1. Loading the task description
2. Adding prior learnings (including from the blocked attempt)
3. Adding the human's answer
4. The agent starts fresh but with all the context it needs

The worktree contains the previous agent's file changes, so the new agent can see the work in progress.

```rust
let mut learnings = prior_learnings.to_vec();
learnings.push(format!(
    "Previous attempt was blocked waiting for a human answer. \
     The human answered: '{}'. Continue from the existing state of the codebase.",
    answer
));
```

**Trade-offs:**
- (+) Simple — just a regular step with extra context
- (+) No dependency on SDK session resume
- (+) Fresh context window (no accumulation from previous attempt)
- (+) Works with the existing retry infrastructure
- (-) The new agent has to rediscover the codebase state (time and tokens)
- (-) May take a different approach than the previous agent
- (-) Prior work is only visible through the worktree files, not through conversation memory

### Approach C: No Resume — Manual Re-Run

The human reviews the queue, answers questions, and then manually re-runs the session:

```bash
# Answer questions
rloop queue review

# Re-run the task (answers are now available)
rloop run 05
```

The task file still has `completed: false`, so it will be re-executed. The session loads answered questions from the queue directory and injects them into the prompt.

**Trade-offs:**
- (+) Simplest — no resume logic at all
- (+) Clean mental model: review, then run
- (+) No session state management
- (-) Human must manually trigger the re-run
- (-) Doesn't work with daemon mode (daemon should auto-resume)
- (-) The entire DAG is re-evaluated (though completed tasks are skipped)

### Recommended Approach

**Approach C (no resume, manual re-run) for v1.** The human reviews the queue, then runs `rloop run` again. Answered questions are injected into the prompt.

**Approach B (rebuild context) for v1.1.** When fake-blocking is implemented, a blocked task can be automatically retried with the answer in the prompt. Still requires manual `rloop run` but the blocked task is specifically targeted.

**Approach A (session resume) is deferred indefinitely** unless the SDK makes it trivially easy. The complexity isn't justified given that Approach B works well enough.

### Daemon Integration (v2)

In daemon mode, resume is natural:

```
Agent asks blocking question
  → Step returns Blocked
  → Daemon records blocked task + queue item ID
  → Daemon continues with other tasks in the DAG

Human answers question
  → Queue item status changes to Answered
  → Daemon detects the change (filesystem watch or polling)
  → Daemon re-runs the blocked task with the answer
```

The daemon already needs to manage task scheduling. Adding "re-run blocked tasks when answers arrive" is a natural extension of the scheduling logic.

---

## 6. Trade-Off Summary

| Dimension | V1 Choice | V1.1 Upgrade | V2 (Daemon) |
|-----------|-----------|--------------|-------------|
| **Blocking semantics** | Non-blocking only | Fake blocking (end step, retry with answer) | True blocking with daemon auto-resume |
| **Queue storage** | File-based (`.rloop/queue/*.json`) | Same | SQLite (concurrent access) |
| **Human interface** | CLI prompts (`rloop queue review`) | TUI (ratatui) | TUI + optional web UI |
| **Answer processing** | Raw answers saved as-is | LLM processes answers, generates follow-ups | Same + auto-dispatch |
| **Session resume** | Manual re-run (`rloop run`) | Auto-retry blocked tasks within session | Daemon auto-resume on answer |
| **DAG interaction** | Questions are informational only | Blocking questions yield the step | Blocked tasks re-queued automatically |

### Complexity vs Value

| Feature | Complexity | Value | Risk |
|---------|------------|-------|------|
| Non-blocking `ask_question` | Low | Medium (agents can flag uncertainties) | Low |
| Non-blocking `log_assumption` | Low | Medium (audit trail of agent decisions) | Low |
| Non-blocking `propose_task` | Medium (task creation logic) | High (agents discover work) | Low |
| CLI queue review | Low | High (humans can batch-review) | Low |
| Fake blocking with retry | Medium | Medium (cleaner than non-blocking) | Medium |
| TUI review interface | High | Medium (nicer UX, same functionality) | Medium |
| LLM answer processing | Medium | Low-Medium (nice-to-have for v1) | Medium |
| True blocking + resume | High | Medium (only matters with daemon) | High |
| Daemon auto-resume | High | High (fully autonomous loop) | High |

---

## 7. Implementation Sketch

### New Files

- `rloop/src/queue.rs` — queue item types, file-based storage, load/save/expire
- `rloop/src/queue_tools.rs` — MCP tool definitions for `ask_question`, `log_assumption`, `propose_task`

### Changes to Existing Files

- `rloop/src/tools.rs` — integrate queue tools into the MCP server alongside `complete` and `context_usage`
- `rloop/src/step.rs` — pass queue state to tools, load answers into prompt
- `rloop/src/session.rs` — expire queue items on task completion/failure, load answers for retries
- `rloop/src/config.rs` — add `[queue]` config section (enabled, cleanup_days)
- `rloop/src/main.rs` — add `rloop queue review`, `rloop queue list`, `rloop queue stats` commands
- `rloop/src/log.rs` — new event types for queue operations

### New Event Types

```rust
// In log.rs Event enum:
QueueItemCreated {
    item_id: String,
    item_type: String,  // "question", "assumption", "proposed_task"
    task_id: TaskId,
    blocking: bool,
},
QueueItemAnswered {
    item_id: String,
    task_id: TaskId,
},
QueueItemExpired {
    item_id: String,
    task_id: TaskId,
    reason: String,     // "task_completed", "task_failed", "session_ended"
},
QueueReviewStarted {
    item_count: u32,
},
QueueReviewCompleted {
    answered: u32,
    skipped: u32,
    approved: u32,
    rejected: u32,
},
```

### Config Section

```toml
# .rloop/config.toml
[queue]
enabled = true           # enable agent queue tools
cleanup_days = 7         # auto-cleanup expired items older than this
```

### CLI Commands

```rust
enum QueueAction {
    Review {
        #[arg(long)]
        task: Option<u32>,
    },
    List {
        #[arg(long)]
        status: Option<String>,  // "pending", "answered", "all"
    },
    Stats,
}
```

### Tool Registration

The queue tools need access to the queue directory and task context:

```rust
pub fn create_tools(
    verification: Option<&VerificationCommands>,
    worktree_path: impl AsRef<Path>,
    timeout_seconds: u64,
    queue_dir: Option<PathBuf>,   // NEW: None if queue is disabled
    task_id: TaskId,               // NEW: for tagging queue items
    session_id: SessionId,         // NEW: for tagging queue items
) -> Tools {
    // ... existing complete + context_usage tools ...

    if let Some(queue_dir) = queue_dir {
        let ask_question = build_ask_question_tool(queue_dir.clone(), task_id, session_id.clone());
        let log_assumption = build_log_assumption_tool(queue_dir.clone(), task_id, session_id.clone());
        let propose_task = build_propose_task_tool(queue_dir, task_id, session_id);
        // Add to MCP server tool list
    }
}
```

The `allowed_tools` list in `compute_tool_lists` gains three new entries when the queue is enabled:

```rust
allowed.push("mcp__rloop__ask_question".to_string());
allowed.push("mcp__rloop__log_assumption".to_string());
allowed.push("mcp__rloop__propose_task".to_string());
```

### Prompt Updates

When queue tools are available, the system prompt should mention them:

```rust
fn build_prompt(task: &Task, prior_learnings: &[String], queue_answers: &[QueueAnswer], queue_enabled: bool) -> String {
    // ... existing prompt ...

    if queue_enabled {
        prompt.push_str(
            "\n## Communication Tools\n\n\
             You have tools to communicate with the human:\n\
             - `ask_question`: Ask a question. Set `blocking: true` if you can't proceed without an answer.\n\
             - `log_assumption`: Record an assumption you're making.\n\
             - `propose_task`: Suggest a new task you've discovered.\n\
             \n\
             Prefer making reasonable assumptions over asking blocking questions. \
             Use `log_assumption` liberally — it helps the human understand your reasoning.\n"
        );
    }

    // ... queue answers section ...
}
```

---

## 8. Open Questions

### Must Resolve Before Implementation

1. **Should queue tools be enabled by default?** If the queue is always on, every agent has three extra tools. This slightly reduces the agent's focus. Recommendation: enabled by default, but the tools are only useful if someone runs `rloop queue review`. An unused queue just accumulates items that expire.

2. **Should `propose_task` actually create task files?** Two options: (a) `propose_task` creates a draft in `.rloop/queue/` that the human approves via `rloop queue review`, then it becomes a real task file; (b) `propose_task` creates the task file directly and the human reviews it as a normal task. Recommendation: (a) — draft in queue, approved via review. This prevents agents from creating arbitrary tasks without human oversight.

3. **How does `ask_question(blocking=true)` interact with `max_retries`?** If a blocking question causes the step to end, should that count as a retry attempt? Recommendation: No. A blocked step is not a failed step. Introduce a separate counter or just don't count blocked outcomes against `max_retries`.

4. **What if the agent asks the same question on every retry?** Without conversation memory, a retried agent might ask the same question again even if it's already been answered. Mitigation: answered questions are injected into the prompt, so the agent should see "Human Answers" and not re-ask. But this depends on prompt engineering.

5. **Should the queue persist across sessions?** Yes. A question asked in session A should still be answerable and consumable in session B. The queue is a per-project resource, not per-session. Queue items are tagged with `session_id` for provenance but not scoped to it.

### Must Resolve Before Building Blocking

6. **Fake blocking or true blocking?** Recommendation: fake blocking (end step, retry with answer) for v1.1. True blocking requires daemon mode. See section 1 for full analysis.

7. **Should blocked tasks block their DAG dependents?** Yes, logically. If task 03 is blocked on a question and task 05 depends on 03, task 05 should not run until 03 completes. However, tasks independent of 03 should still proceed. This is the same logic as failed tasks blocking dependents.

### Must Resolve Before Building TUI

8. **Is ratatui the right framework?** It's the dominant Rust TUI framework and actively maintained. Alternatives: `cursive` (higher-level but less flexible), `dioxus-tui` (React-like but experimental). Recommendation: ratatui. It has the largest ecosystem and best documentation.

9. **How does the TUI handle long-running LLM processing?** When answer processing runs in the background, the TUI needs to show a spinner or progress indicator. This requires async event handling in the TUI loop. Ratatui supports this via `crossterm` events + tokio channels.

### Worth Discussing

10. **Should there be a notification system?** When the daemon is running and a question is asked, should the human get a desktop notification? This is a v2+ feature but worth designing for. The queue review command could be triggered by a notification.

11. **Should humans be able to review completed task code through the queue?** Beyond questions, assumptions, and proposed tasks, the queue could include "code review" items — snapshots of completed task diffs for optional human review before merging. This overlaps with the session branch review workflow but could provide a more structured review experience.

12. **How does the queue interact with parallel DAG execution (research 02)?** Multiple agents running in parallel might all queue items simultaneously. With file-based storage, concurrent writes need careful handling (unique filenames, atomic writes). The file-naming convention (`{timestamp}-{uuid}.json`) handles this, but it's worth testing under parallel load.

13. **Should queue items include the agent's worktree diff?** When the human reviews a question, seeing what the agent has done so far (via diff) would provide valuable context. This could be captured when the queue item is created: `git diff` in the worktree at the time of the question.

14. **How does this interact with LLM verification (research 03)?** If an agent asks a blocking question and the step ends, LLM verification doesn't run (the step didn't call `complete`). This is correct — blocked steps aren't complete. But when the step is retried with the answer and succeeds, normal verification applies.
