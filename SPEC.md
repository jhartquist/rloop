# rloop — Technical Spec

## Development Phases

Build top-down, implement bottom-up. Each phase is a checkpoint for human review.

### Phase 1: Type Skeleton + Signatures

Define every public type and function signature across all modules. Everything compiles. All function bodies are `todo!()`. This is the architecture — the shape of the entire system visible at once.

**Deliver:** All modules with types and `todo!()` functions. `cargo check` passes. `cargo clippy` passes.

**Review:** Are the types right? Are the module boundaries clean? Is the public API minimal?

### Phase 2: Tests

Write tests against the interfaces from Phase 1. Tests compile but fail (because implementations are `todo!()`).

Focus areas:
- Task parsing (frontmatter, markdown body, edge cases)
- DAG resolution (linear chain, diamond, cycle detection, single task with no deps)
- Worktree lifecycle (create, lock, merge, cleanup)
- Session orchestration (skip completed deps, abort on failure)
- Event logging (correct events emitted in correct order)

**Deliver:** Comprehensive test suite. `cargo test` compiles but all tests fail.

**Review:** Do the tests cover the important cases? Are we testing behavior, not implementation?

### Phase 3: Implementation

Fill in `todo!()`s bottom-up, module by module. Each module is a sub-checkpoint.

**Order:**
1. `config.rs` — load config, merge with defaults
2. `task.rs` — parse task files, resolve DAG
3. `git.rs` — worktree create, lock, merge, cleanup
4. `log.rs` — event types, JSONL writer, terminal printer
5. `tools.rs` — MCP complete tool
6. `step.rs` — single step execution (agent + verify + retry)
7. `session.rs` — orchestrate DAG execution
8. `main.rs` — CLI entry point

Each module: implement, make its tests pass, clippy clean, commit. Move to next.

**Review:** After each module, verify tests pass and code is clean.

### Phase 4: Examples + Integration

Wire everything together with runnable examples and integration tests.

```
examples/
├── 01-parse-task.rs        # Load and print parsed task
├── 02-resolve-dag.rs       # Show DAG execution order
├── 03-single-step.rs       # Run one task in a worktree
├── 04-session.rs           # Full session with DAG
```

**Deliver:** Working end-to-end flow. Examples run successfully.

## Core Types

### task.rs

```rust
/// A task loaded from a markdown file with YAML frontmatter.
pub struct Task {
    pub id: String,
    pub model: Model,
    pub depends_on: Vec<String>,
    pub verification: Option<Vec<String>>,
    pub completed: bool,
    pub description: String,           // markdown body
    pub source_path: PathBuf,          // where it was loaded from
}

/// Ordered execution plan from DAG resolution.
pub struct ExecutionPlan {
    pub target: String,                // the task that was requested
    pub steps: Vec<String>,            // task IDs in execution order
}

pub fn load_task(path: &Path) -> Result<Task>;
pub fn load_all_tasks(dir: &Path) -> Result<Vec<Task>>;
pub fn resolve_dag(tasks: &[Task], target: &str) -> Result<ExecutionPlan>;
```

### step.rs

```rust
/// Outcome of a single step (one execution attempt of a task).
pub enum StepOutcome {
    Success { summary: String },
    Failed { reason: String, learnings: String },
}

/// Result of executing a task (may include multiple retry attempts).
pub struct StepResult {
    pub task_id: String,
    pub outcome: StepOutcome,
    pub attempts: u32,
    pub duration: Duration,
}

pub async fn execute_step(
    task: &Task,
    worktree_path: &Path,
    config: &Config,
    prior_learnings: &[String],
) -> Result<StepResult>;
```

### git.rs

```rust
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub task_id: String,
}

pub fn create_worktree(repo: &Path, task_id: &str, base_branch: &str) -> Result<Worktree>;
pub fn lock_worktree(worktree: &Worktree) -> Result<()>;
pub fn unlock_worktree(worktree: &Worktree) -> Result<()>;
pub fn merge_worktree(repo: &Path, worktree: &Worktree, into_branch: &str) -> Result<()>;
pub fn cleanup_worktree(repo: &Path, worktree: &Worktree) -> Result<()>;
```

### session.rs

```rust
pub struct SessionResult {
    pub branch: String,
    pub completed_tasks: Vec<String>,
    pub failed_task: Option<String>,
    pub auto_merged: bool,
}

pub async fn run_session(
    repo: &Path,
    target_task: &str,
    config: &Config,
    auto_merge: bool,
) -> Result<SessionResult>;
```

### log.rs

```rust
/// All events that can occur during a session.
pub enum Event {
    // rloop lifecycle
    SessionStarted { session_id: String, target: String },
    TaskStarted { task_id: String },
    WorktreeCreated { task_id: String, path: PathBuf },
    PromptSent { task_id: String },
    VerificationRan { task_id: String, command: String, passed: bool, output: String },
    TaskCompleted { task_id: String, summary: String },
    TaskFailed { task_id: String, reason: String },
    WorktreeMerged { task_id: String, into_branch: String },
    WorktreeCleanedUp { task_id: String },
    SessionComplete { branch: String },
    SessionAutoMerged { branch: String, into_branch: String },
    SessionCancelled,

    // Claude stream events
    AssistantMessage { content: String },
    ToolCall { name: String, input: serde_json::Value },
    ToolResult { name: String, output: serde_json::Value },
    TokenUsage { input_tokens: u64, output_tokens: u64, cache_read: u64, cache_creation: u64 },
    ContextUsage { percentage: f32 },
}

pub struct SessionLogger { /* ... */ }

impl SessionLogger {
    pub fn new(session_dir: &Path, session_id: &str) -> Result<Self>;
    pub fn log(&mut self, event: &Event) -> Result<()>;         // write to JSONL
    pub fn print(&self, event: &Event);                          // pretty terminal output
}
```

### config.rs

```rust
pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
    pub session_dir: PathBuf,
}

pub fn load_config(rloop_dir: &Path) -> Result<Config>;
pub fn init_rloop_dir(repo: &Path) -> Result<PathBuf>;
```

### tools.rs

```rust
/// Create the `complete` MCP tool for signaling task completion.
pub fn create_complete_tool(
    verification_commands: Option<&[String]>,
    worktree_path: &Path,
) -> /* MCP tool type */;
```

### Model (shared)

```rust
pub enum Model {
    Haiku,
    Sonnet,
    Opus,
}
```

## Module Dependency Graph

```
main.rs
  └── session.rs
        ├── task.rs        (load tasks, resolve DAG)
        ├── step.rs        (execute individual tasks)
        │     ├── tools.rs (MCP complete tool)
        │     └── log.rs   (log Claude events)
        ├── git.rs         (worktree lifecycle)
        ├── log.rs         (log session events)
        └── config.rs      (load configuration)
```

Clean tree — no circular dependencies. Lower modules know nothing about higher ones.

## SDK Reference (claude-agent-sdk-rs)

Patterns from rstep v1 (`archive/rstep-era/rstep_v1/`). Reference that code for working examples.

### Client Setup

```rust
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
use futures::StreamExt;

let options = ClaudeAgentOptions::builder()
    .model(model.to_string())           // "Haiku", "Sonnet", "Opus"
    .cwd(worktree_path)                 // sandboxed working directory
    .setting_sources(vec![              // project settings only, no global
        "project".to_string(),
    ])
    .allowed_tools(allowed)             // Vec<String> of tool names
    .disallowed_tools(disallowed)       // Vec<String> of tool names
    .mcp_servers(McpServers::Dict(servers))  // custom MCP tools
    .output_format(schema)             // structured output (for failure fallback)
    .build();

let mut client = ClaudeClient::new(options);
client.connect().await?;
client.query(prompt).await?;
```

**Key details:**
- `.setting_sources(vec![])` disables ALL settings (no CLAUDE.md, no skills)
- `.setting_sources(vec!["project".to_string()])` loads project-level settings only
- `.allowed_tools(vec![])` with no disallowed still keeps defaults — must explicitly disable
- Always call `.connect()` before `.query()`

### Tool Allowlisting

Use the `BuiltinTool` enum to compute allowed/disallowed sets:

```rust
use strum::IntoEnumIterator;
use std::collections::HashSet;

let allowed: HashSet<BuiltinTool> = [
    BuiltinTool::Read, BuiltinTool::Write, BuiltinTool::Edit,
    BuiltinTool::Bash, BuiltinTool::Glob, BuiltinTool::Grep,
    BuiltinTool::WebFetch, BuiltinTool::WebSearch,
].into_iter().collect();

let disallowed: HashSet<BuiltinTool> = BuiltinTool::iter()
    .collect::<HashSet<_>>()
    .difference(&allowed)
    .copied()
    .collect();

// Convert to Vec<String> for the builder
// Add custom MCP tools to allowed: "mcp__rloop__complete"
```

MCP tool naming convention: `mcp__<server_name>__<tool_name>`

### Streaming Messages

```rust
let mut stream = client.receive_response();
while let Some(result) = stream.next().await {
    let msg = result?;
    match &msg {
        Message::System(m) => {
            // First message. m.session_id, m.model, m.tools
        }
        Message::Assistant(m) => {
            // Claude output. m.message.content is Vec<ContentBlock>
            for block in &m.message.content {
                match block {
                    ContentBlock::Text(t) => { /* t.text */ }
                    ContentBlock::ToolUse(tu) => { /* tu.name, tu.input */ }
                    _ => {}
                }
            }
        }
        Message::User(_) => {
            // Tool results flowing back. Access via .extra field (SDK quirk)
        }
        Message::Result(m) => {
            // Final message. m.total_cost_usd, m.duration_ms, m.num_turns
            // m.result contains the text response
            // m.structured_output if output_format was set
        }
        _ => {}
    }
}
client.disconnect().await?;
```

**Message flow:** System -> (Assistant <-> User)* -> Result

### Defining MCP Tools

Use `schemars` to derive tool input schemas from Rust types:

```rust
use claude_agent_sdk_rs::{tool, ToolResult, McpToolResultContent, create_sdk_mcp_server};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

/// Input schema for the complete tool — derived with schemars.
#[derive(Deserialize, JsonSchema)]
struct CompleteInput {
    /// Brief summary of what was done
    summary: String,
}

let summary: Arc<Mutex<Option<String>>> = Arc::default();
let stop: Arc<AtomicBool> = Arc::default();

let summary_clone = Arc::clone(&summary);
let stop_clone = Arc::clone(&stop);

let schema = serde_json::to_value(schemars::schema_for!(CompleteInput))?;

let complete_tool = tool!(
    "complete",
    "Call when you have finished the task.",
    schema,
    move |args: serde_json::Value| {
        let summary_inner = Arc::clone(&summary_clone);
        let stop_inner = Arc::clone(&stop_clone);
        async move {
            let text = args.get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            *summary_inner.lock().unwrap() = Some(text);
            stop_inner.store(true, Ordering::SeqCst);

            Ok(ToolResult {
                content: vec![McpToolResultContent::Text {
                    text: "Task complete.".into(),
                }],
                is_error: false,
            })
        }
    }
);

// Register in MCP server
let server = create_sdk_mcp_server("rloop", "0.1.0", vec![complete_tool]);
let mut servers = HashMap::new();
servers.insert("rloop".to_string(), McpServerConfig::Sdk(server));
```

**Verification in the tool:** Return `is_error: true` to make Claude retry:

```rust
if !tests_passed {
    Ok(ToolResult {
        content: vec![McpToolResultContent::Text {
            text: format!("Tests failed:\n{stderr}"),
        }],
        is_error: true, // triggers Claude retry
    })
}
```

### Stream Interruption

When the `complete` tool fires, interrupt Claude but keep reading for the Result message:

```rust
let mut interrupted = false;
while let Some(result) = stream.next().await {
    let msg = result?;
    // ... handle message ...

    if !interrupted && stop.load(Ordering::SeqCst) {
        client.interrupt().await.ok();
        interrupted = true;
        // DON'T break — keep reading to receive Result with cost/duration
    }
}
```

### Structured Output (Failure Fallback)

Use `schemars` to derive JSON schemas from Rust types. The structured output schema maps directly to `StepOutcome::Failed`:

```rust
use schemars::JsonSchema;
use serde::Deserialize;

/// Schema for structured output — only used when agent fails without calling `complete`.
#[derive(Deserialize, JsonSchema)]
pub struct FailureOutput {
    pub reason: String,
    pub learnings: Vec<String>,
}

// Generate schema at runtime
let schema = schemars::schema_for!(FailureOutput);
let output_format = json!({
    "type": "json_schema",
    "schema": schema,
});
```

**Design:** Two completion paths, one type system:
- **Success:** agent calls `complete` tool → `StepOutcome::Success { summary }`
- **Failure:** agent exits without `complete` → structured output deserializes to `FailureOutput` → `StepOutcome::Failed { reason, learnings }`

No hand-written JSON schemas. `schemars` derives them from the types we already have.

Access after stream ends:

```rust
if let Message::Result(res) = last_message {
    if let Some(output) = &res.structured_output {
        let failure: FailureOutput = serde_json::from_value(output.clone())?;
        // → StepOutcome::Failed { reason: failure.reason, learnings: failure.learnings }
    }
}
```

### Token / Cost Extraction

From the final `Result` message:

```rust
if let Message::Result(res) = last_message {
    let cost = res.total_cost_usd.unwrap_or(0.0);
    let duration_ms = res.duration_ms;
    let turns = res.num_turns;
}
```

Per-message token tracking is available on `Assistant` messages via usage fields — accumulate these for context percentage calculation.

## Task File Parsing

Task files are markdown with YAML frontmatter delimited by `---`:

```markdown
---
id: "03"
model: opus
depends_on: ["01", "02"]
verification: ["cargo test"]
completed: false
---

# Task title

Description body in markdown.
```

**Parsing approach:**
1. Split file on first two `---` lines to extract YAML block
2. Deserialize YAML frontmatter with `serde_yaml` into a `TaskFrontmatter` struct
3. Remainder after second `---` is the markdown description (trim leading whitespace)
4. Combine into `Task` struct

```rust
#[derive(Deserialize)]
struct TaskFrontmatter {
    id: String,
    #[serde(default = "default_model")]
    model: Model,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    verification: Option<Vec<String>>,
    #[serde(default)]
    completed: bool,
}
```

## Signal Handling (Ctrl+C)

Use `tokio::signal` for graceful shutdown:

```rust
tokio::select! {
    result = run_session(...) => { /* normal completion */ }
    _ = tokio::signal::ctrl_c() => {
        // cleanup all active worktrees
        // log SessionCancelled event
        // exit cleanly
    }
}
```

Worktree cleanup must be synchronous (git commands) and happen even on panic. Consider a cleanup guard or `Drop` impl on a session handle.
