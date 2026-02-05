# rstep - The Step Executor

`rstep` is a single-task executor. It runs one step of the rloop MDP.

## What It Does

```
rstep [task-id]

1. Get task (from br, or use provided ID)
2. Check for blockers (human queue items)
3. Create worktree with task branch
4. Run Claude with controlled environment
5. Verify (tests, build)
6. On success: merge to main, cleanup
7. Return: Success | Failed | Blocked
```

## CLI

```bash
# Run next ready task
rstep

# Run specific task
rstep bd-abc123

# Run without merge (for parallel orchestration later)
rstep --no-merge

# Dry run - show what would execute
rstep --dry-run
```

## StepResult

```rust
enum StepResult {
    Success {
        summary: String,
        tokens_used: u64,
        duration_ms: u64,
    },
    Failed {
        reason: String,
        next_steps: Vec<String>,
        tokens_used: u64,
    },
    Blocked {
        blocker: Blocker,
    },
}

enum Blocker {
    Question { id: String, text: String },
    Assumption { id: String, needs_review: bool },
    Approval { id: String, proposed_task: String },
    Dependency { task_id: String },
}
```

## Environment Setup

### Bare Minimum Start

Each step starts with maximum control:

```
temp_dir/           # Empty directory
├── (task files)    # Only what the task needs
└── (nothing else)  # No CLAUDE.md, no skills, no ambient context
```

We inject ONLY what we explicitly choose:
- Task description
- Relevant context (if any)
- Tool definitions
- System prompt

### Tool Control

**Disable all by default.** Explicit allowlist:

```rust
let allowed_tools = vec![
    "read",
    "write",
    "edit",
    "bash",
    "glob",
    "grep",
];
// No web_search, no web_fetch, no skills, no MCP servers
```

### Filesystem Sandboxing

Claude can ONLY access the working directory:

```rust
// Security tests verify:
assert!(can_read("./src/main.rs"));           // OK
assert!(!can_read("/tmp/other-project/"));    // BLOCKED
assert!(!can_read("~/.ssh/id_rsa"));          // BLOCKED
assert!(!can_read("../sibling-dir/"));        // BLOCKED
```

## Token Tracking

Track tokens from every stream message:

```rust
struct TokenMetrics {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
}

// Accumulated over the session
struct SessionMetrics {
    total_input: u64,
    total_output: u64,
    context_percentage: f32,  // How full is the context window?
    messages: Vec<MessageMetrics>,
}
```

After each message, we know:
- Total tokens used
- Context utilization percentage
- Rate of context growth

This enables:
- Early warning when approaching context limits
- Analysis of which tasks consume most context
- Model selection based on expected context needs

## Worktree Lifecycle

```
1. CREATE
   git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}

2. LOCK (during work)
   git worktree lock .rloop/worktrees/{task-id} --reason "rstep active"

3. WORK
   Claude operates in worktree directory
   All file operations sandboxed here

4. VERIFY
   Run verification commands in worktree
   cargo check, cargo test, etc.

5. MERGE (if success and --no-merge not set)
   git checkout main
   git merge --squash rloop/{task-id}
   git commit -m "task: {summary}"
   git push (with retry/rebase on conflict)

6. CLEANUP
   git worktree unlock .rloop/worktrees/{task-id}
   git worktree remove .rloop/worktrees/{task-id}
   git branch -d rloop/{task-id}
```

## Structured Output

Instead of MCP tools for completion, use structured output:

```rust
// Required final message format
#[derive(Deserialize)]
struct TaskOutput {
    result: TaskOutcome,
    summary: String,
    #[serde(default)]
    next_steps: Vec<String>,
}

#[derive(Deserialize)]
enum TaskOutcome {
    Success,
    Failed { reason: String },
    Blocked { blocker_type: String, details: String },
}
```

System prompt enforces this format for the final message.

## Session Logging

Every step produces a complete session log:

```rust
struct SessionLog {
    // Identity
    task_id: TaskId,
    session_id: SessionId,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,

    // Config (policy)
    config: StepConfig,

    // Trajectory
    events: Vec<SessionEvent>,

    // Outcome (reward)
    result: StepResult,

    // Metrics
    metrics: SessionMetrics,
}

enum SessionEvent {
    // Claude output
    AssistantMessage { content: String, tokens: TokenMetrics },

    // Tool usage
    ToolCall { name: String, input: Value },
    ToolResult { name: String, output: Value },

    // Human queue
    QuestionAsked { id: String, text: String, blocking: bool },
    AssumptionLogged { id: String, decision: String },
    TaskProposed { id: String, title: String },

    // Lifecycle
    VerificationStarted { commands: Vec<String> },
    VerificationResult { passed: bool, output: String },
}
```

Stored as JSONL: `.rloop/sessions/{session-id}.jsonl`

## Integration

### br (beads_rust)

Shell out to `br` CLI:

```rust
// Get next ready task
let task = br_ready()?;  // br ready --json --limit 1

// Mark in progress
br_update(&task.id, Status::InProgress)?;

// On success
br_close(&task.id, &summary)?;

// On failure (for retry)
br_update(&task.id, Status::Open)?;
```

### hq (human queue) - Future

When we add human queue:

```rust
// Check for blockers before starting
if hq_has_blocking(&task.id)? {
    return StepResult::Blocked { ... };
}

// Agent can queue items during execution
hq_ask(question, blocking)?;
hq_assume(decision, reasoning)?;
hq_propose(task_title, description)?;
```

## Recording & Replay

For deterministic testing:

```rust
// Record a real session
let recorder = RecordingAgent::new(real_agent, "fixtures/task-123.json");
recorder.run(task).await?;

// Replay for tests
let recorded = RecordedAgent::from_file("fixtures/task-123.json")?;
let result = recorded.run(task).await?;
assert_eq!(result, expected);
```

Fixtures stored in `fixtures/recorded/`.

## Configuration

```toml
# .rloop/config.toml

[step]
model = "claude-sonnet-4-20250514"
max_turns = 50

[step.tools]
allowed = ["read", "write", "edit", "bash", "glob", "grep"]

[verification]
commands = ["cargo check", "cargo test"]
fail_fast = true

[worktree]
root = ".rloop/worktrees"
auto_merge = true  # Set false when rloop orchestrates

[logging]
session_dir = ".rloop/sessions"
level = "debug"
```
