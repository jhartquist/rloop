# rstep Examples Plan

Chronological examples building up the step executor. This is a loose plan, feel free to insert any examples, change the order, or modify the existing ones to improve the flow.

## Example 00: Minimal Query (Verbose)

**Goal:** Absolute minimum - call Claude, get response, dump EVERYTHING.

**What it does:**
1. Create temp directory
2. `cd` into it (set working_directory)
3. Call Claude with "What is 2+2?"
4. Print EVERY message as raw debug output (or JSON for piping to `jq`)
5. Accumulate and print token counts
6. Delete temp directory
7. Exit

**Output:** Raw/verbose - every field of every message. Pipe to `jq` for readability:
```bash
cargo run --example 00-minimal-query 2>&1 | jq .
```

**Disabled:**
- All tools (empty tools list)
- All settings/skills (empty setting_sources)
- No CLAUDE.md

**Key code pattern:**
```rust
use claude_agent_sdk_rs::{ClaudeClient, ClaudeAgentOptions, Message};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let temp = tempdir()?;

    let options = ClaudeAgentOptions::builder()
        .working_directory(temp.path())
        .tools([])                   // No tools
        .setting_sources(vec![])     // No settings/skills
        .build();

    let mut client = ClaudeClient::new(options);
    client.connect().await?;
    client.query("What is 2+2?").await?;

    let mut total_input = 0u64;
    let mut total_output = 0u64;

    let mut stream = client.receive_response();
    while let Some(msg) = stream.next().await {
        match msg? {
            Message::Assistant(m) => {
                // Print content
                // Accumulate tokens from m.message.usage
            }
            Message::Result(r) => {
                // Print final usage from r.usage
                break;
            }
            _ => {}
        }
    }

    client.disconnect().await?;
    // temp dir auto-deleted on drop
    Ok(())
}
```

**Commits:**
1. `ex-00: minimal query with verbose output`
   - Adds: tokio, anyhow, serde_json, futures, tempfile
   - Creates: examples/00-minimal-query.rs

---

## Example 01: Pretty Output

**Goal:** Make the output readable - colors, formatting.

**Adds:**
- `tracing` + `tracing-subscriber` for structured logging
- Colored output (maybe `colored` crate or just ANSI)
- Clear sections: "Starting...", "Response:", "Tokens:"

**Commits:**
1. `deps: add tracing, tracing-subscriber`
2. `ex-01: pretty formatted output (N lines)`

---

## Example 02: Working Directory Setup

**Goal:** Proper temp directory with template copying.

**Adds:**
- Copy `templates/bug-adder/` into temp dir
- Run query in context of that project
- Still no tools - Claude can't do anything yet

**Commits:**
1. `ex-02: copy template to temp dir (N lines)`

---

## Example 03: Enable Read Tool

**Goal:** Let Claude read files.

**Adds:**
- Enable just `Read` tool
- Ask Claude "What does the add function do?"
- Claude can now read src/lib.rs

**Commits:**
1. `ex-03: enable Read tool (N lines)`

---

## Example 04: Enable Write/Edit Tools

**Goal:** Let Claude modify files.

**Adds:**
- Enable `Read`, `Write`, `Edit` tools
- Ask Claude "Fix the bug in the add function"
- Claude can now fix the code

**Commits:**
1. `ex-04: enable Write and Edit tools (N lines)`

---

## Example 05: Verification

**Goal:** Run `cargo test` after Claude makes changes.

**Adds:**
- Enable `Bash` tool (or run externally after)
- After Claude finishes, run `cargo test` in temp dir
- Report pass/fail

**Commits:**
1. `ex-05: run cargo test for verification (N lines)`

---

## Example 06: Structured Output

**Goal:** Require JSON result format.

**Adds:**
- System prompt requiring `StepResult` JSON as final message
- Parse and validate the response
- `StepResult { success: bool, summary: String }`

**Commits:**
1. `ex-06: structured output parsing (N lines)`

---

## Example 07: StepResult Enum

**Goal:** Full result type with Success/Failed/Blocked.

**Adds:**
```rust
enum StepResult {
    Success { summary: String },
    Failed { reason: String, next_steps: Vec<String> },
    Blocked { blocker: String },
}
```

**Commits:**
1. `ex-07: StepResult enum (N lines)`

---

## Example 08: Session Logging

**Goal:** Log everything to JSONL file.

**Adds:**
- Write all events to `session.jsonl`
- Include timestamps, token counts per message
- Queryable history

**Commits:**
1. `deps: add chrono, serde`
2. `ex-08: session logging to JSONL (N lines)`

---

## Example 09: Recording

**Goal:** Record Claude responses for replay.

**Adds:**
- Save full session to fixture file
- Based on rloop's RecordingAgent pattern

**Commits:**
1. `ex-09: record sessions to fixtures (N lines)`

---

## Example 10: Replay

**Goal:** Replay recorded sessions for tests.

**Adds:**
- Load fixture, replay without calling Claude
- Deterministic, fast, free tests

**Commits:**
1. `ex-10: replay from recorded fixtures (N lines)`

---

## Example 11: Integration Test

**Goal:** Full test using recording/replay.

**Adds:**
- Test that runs against bug-adder template
- Uses recorded fixture
- Asserts StepResult::Success

**Commits:**
1. `ex-11: integration test with replay (N lines)`

---

## Example 12: br Integration

**Goal:** Get task from beads_rust.

**Adds:**
- Shell out to `br ready --json --limit 1`
- Parse task
- Use task description as prompt

**Commits:**
1. `ex-12: br integration (N lines)`

---

## Example 13: Custom MCP Tool

**Goal:** Add our first custom tool.

**Adds:**
- Simple MCP tool (maybe `step_complete`)
- Register with ClaudeClient
- Claude can call it

**Commits:**
1. `ex-13: custom MCP tool (N lines)`

---

## Example 14: Security Boundaries

**Goal:** Verify Claude cannot escape the working directory.

**What it does:**
1. Set up sandbox in temp directory
2. Ask Claude to read files outside the sandbox (parent dir, home, /etc)
3. Verify all attempts are blocked or fail
4. Confirm Claude can still read files inside the sandbox

**Key tests:**
```rust
// Claude should NOT be able to:
- Read "../sibling-project/secrets.txt"
- Read "~/.ssh/id_rsa"
- Read "/etc/passwd"
- Write to any path outside working directory

// Claude SHOULD be able to:
- Read "./src/lib.rs"
- Write "./src/new_file.rs"
```

**Commits:**
1. `ex-14: security boundary tests (N lines)`

---

## Example 15: Worktree Create

**Goal:** Create a git worktree for isolated task execution.

**What it does:**
1. Initialize a git repo in temp directory
2. Create initial commit on main
3. Create worktree: `git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}`
4. Verify worktree exists and has correct branch
5. Verify main is untouched

**Key code:**
```rust
fn create_worktree(repo_path: &Path, task_id: &str) -> Result<PathBuf> {
    let worktree_path = repo_path.join(".rloop/worktrees").join(task_id);
    let branch_name = format!("rloop/{}", task_id);

    Command::new("git")
        .args(["worktree", "add", &worktree_path.to_string_lossy(), "-b", &branch_name])
        .current_dir(repo_path)
        .output()?;

    Ok(worktree_path)
}
```

**Commits:**
1. `ex-15: worktree create (N lines)`

---

## Example 16: Worktree Lock

**Goal:** Lock worktree during work to prevent conflicts.

**Adds:**
- Lock worktree when work starts
- Unlock when work completes
- Handle lock/unlock errors gracefully

**Key code:**
```rust
fn lock_worktree(worktree_path: &Path, reason: &str) -> Result<()> {
    Command::new("git")
        .args(["worktree", "lock", &worktree_path.to_string_lossy(), "--reason", reason])
        .output()?;
    Ok(())
}

fn unlock_worktree(worktree_path: &Path) -> Result<()> {
    Command::new("git")
        .args(["worktree", "unlock", &worktree_path.to_string_lossy()])
        .output()?;
    Ok(())
}
```

**Commits:**
1. `ex-16: worktree lock/unlock (N lines)`

---

## Example 17: Work in Worktree

**Goal:** Run Claude inside the worktree, not the main repo.

**What it does:**
1. Create worktree from template repo
2. Set Claude's working_directory to worktree path
3. Claude makes edits in the worktree
4. Verify changes are in worktree branch
5. Verify main branch is unchanged

**Key insight:** Claude operates in complete isolation. If the step fails, main is untouched.

**Commits:**
1. `ex-17: work in worktree (N lines)`

---

## Example 18: Commit Changes

**Goal:** Commit Claude's changes in the worktree.

**What it does:**
1. Work in worktree (like ex-17)
2. After Claude finishes, `git add -A && git commit -m "..."`
3. Verify commit exists on task branch
4. Show commit diff

**Commits:**
1. `ex-18: commit changes in worktree (N lines)`

---

## Example 19: Merge to Main

**Goal:** Squash merge successful work back to main.

**What it does:**
1. Complete work in worktree
2. Commit changes
3. Switch to main: `git checkout main`
4. Squash merge: `git merge --squash rloop/{task-id}`
5. Commit with summary: `git commit -m "task: {summary}"`
6. Verify main has the changes

**Commits:**
1. `ex-19: merge to main (N lines)`

---

## Example 20: Worktree Cleanup

**Goal:** Clean up worktree and branch after merge.

**What it does:**
1. Complete full flow: create → work → commit → merge
2. Cleanup sequence:
   - `git worktree unlock .rloop/worktrees/{task-id}`
   - `git worktree remove .rloop/worktrees/{task-id}`
   - `git branch -d rloop/{task-id}`
3. Verify worktree and branch are gone
4. Verify main still has the merged changes

**Commits:**
1. `ex-20: worktree cleanup (N lines)`

---

## Example 21: Full Worktree Lifecycle

**Goal:** Combine all worktree operations into one coherent flow.

**What it does:**
1. CREATE: `git worktree add`
2. LOCK: `git worktree lock`
3. WORK: Claude operates in worktree
4. VERIFY: `cargo test` in worktree
5. MERGE: squash merge to main
6. CLEANUP: unlock, remove worktree, delete branch

This is the complete worktree lifecycle from RSTEP.md.

**Commits:**
1. `ex-21: full worktree lifecycle (N lines)`
2. `ex-21: extract git module to lib`

---

## Example 22: Human Queue - ask_question

**Goal:** Add custom MCP tool for Claude to ask blocking questions.

**What it does:**
1. Define `ask_question` MCP tool
2. Claude can call it when blocked on unclear requirements
3. Tool captures the question and returns "blocked"
4. Step returns `StepResult::Blocked { blocker: Question { ... } }`

**Tool schema:**
```rust
tool!(
    "ask_question",
    "Ask the human a clarifying question. Use when requirements are unclear.",
    json!({
        "type": "object",
        "properties": {
            "question": { "type": "string", "description": "The question to ask" },
            "blocking": { "type": "boolean", "description": "Whether to block until answered" }
        },
        "required": ["question", "blocking"]
    }),
    |args| async move { ... }
)
```

**Commits:**
1. `ex-22: ask_question tool (N lines)`

---

## Example 23: Human Queue - log_assumption

**Goal:** Add tool for Claude to log assumptions for later review.

**What it does:**
1. Define `log_assumption` MCP tool
2. Claude logs decisions made with reasoning
3. Assumptions are queued for human review
4. Non-blocking by default (work continues)

**Tool schema:**
```rust
tool!(
    "log_assumption",
    "Log an assumption you're making. Human will review later.",
    json!({
        "type": "object",
        "properties": {
            "decision": { "type": "string", "description": "What you decided to do" },
            "reasoning": { "type": "string", "description": "Why you made this decision" },
            "needs_review": { "type": "boolean", "description": "Whether human must review before merge" }
        },
        "required": ["decision", "reasoning"]
    }),
    |args| async move { ... }
)
```

**Commits:**
1. `ex-23: log_assumption tool (N lines)`

---

## Example 24: Human Queue - propose_task

**Goal:** Add tool for Claude to propose follow-up tasks.

**What it does:**
1. Define `propose_task` MCP tool
2. Claude can propose new tasks discovered during work
3. Tasks are queued for human approval
4. On approval, tasks are created in br

**Tool schema:**
```rust
tool!(
    "propose_task",
    "Propose a new task you discovered while working. Human will review and approve.",
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string", "description": "Task title" },
            "description": { "type": "string", "description": "What needs to be done and why" },
            "priority": { "type": "integer", "description": "Suggested priority (0-4)" }
        },
        "required": ["title", "description"]
    }),
    |args| async move { ... }
)
```

**Commits:**
1. `ex-24: propose_task tool (N lines)`

---

## Example 25: Combined Human Queue Tools

**Goal:** All three human queue tools working together.

**What it does:**
1. Register all three tools: ask_question, log_assumption, propose_task
2. Give Claude a complex task that might trigger all three
3. Claude asks a question, logs assumptions, proposes follow-up tasks
4. Collect all human queue items
5. Display them for batch review

**Key insight:** The human reviews queue items asynchronously. Multiple tasks can queue items simultaneously.

**Commits:**
1. `ex-25: combined human queue tools (N lines)`
2. `ex-25: extract human_queue module to lib`

---

## Example 26: br Status Updates

**Goal:** Update task status in br during step execution.

**What it does:**
1. Get task from br (like ex-12)
2. Mark in_progress: `br update {id} --status in_progress`
3. Execute step
4. On success: `br close {id} --summary "..."`
5. On failure: `br update {id} --status open` (for retry)
6. On blocked: `br update {id} --status blocked`

**Key code:**
```rust
fn br_mark_in_progress(task_id: &str) -> Result<()> {
    Command::new("br")
        .args(["update", task_id, "--status", "in_progress"])
        .output()?;
    Ok(())
}

fn br_close(task_id: &str, summary: &str) -> Result<()> {
    Command::new("br")
        .args(["close", task_id, "--summary", summary])
        .output()?;
    Ok(())
}
```

**Commits:**
1. `ex-26: br status updates (N lines)`

---

## Example 27: Config File

**Goal:** Load step configuration from `.rloop/config.toml`.

**What it does:**
1. Define config structure matching RSTEP.md
2. Load from `.rloop/config.toml` if present
3. Fall back to defaults
4. Use config values for step execution

**Config structure:**
```toml
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
auto_merge = true

[logging]
session_dir = ".rloop/sessions"
level = "debug"
```

**Commits:**
1. `deps: add toml`
2. `ex-27: config file loading (N lines)`

---

## Example 28: Full Step Flow

**Goal:** Complete step executor - the culmination of all previous examples.

**What it does:**
1. Load config
2. Get task from br
3. Check for blockers (human queue items pending)
4. Create worktree
5. Lock worktree
6. Run Claude with controlled environment and human queue tools
7. On completion, commit changes
8. Run verification (cargo check, cargo test)
9. On success: merge to main, update br status to closed
10. On failure: update br status to open (for retry)
11. On blocked: update br status to blocked
12. Cleanup worktree
13. Log session
14. Return StepResult

**This is the complete rstep flow from RSTEP.md:**
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

**Commits:**
1. `ex-28: full step flow (N lines)`
2. `ex-28: extract step module to lib`

---

## Phase 2: CLI Binary (Examples 29-33)

With examples 00-28 complete, we have a library with:
- `run_step()` - Core step execution
- `Config`, `Task`, `StepResult` - Data types
- `git` module - Worktree operations
- `human_queue` module - Human-in-the-loop tools

Now we turn this into a CLI binary.

### Design Decisions

**Why continue examples vs. a separate phase?**
- Consistency with teaching methodology
- Each example builds on the previous
- CLI is just another layer on the library

**Why clap?**
- Standard choice for Rust CLIs
- Derive macros for minimal boilerplate
- Good error messages out of the box

**Minimal viable CLI:**
```bash
# Run next ready task (from br)
rstep

# Run specific task by ID
rstep bd-abc123

# Options
rstep --dry-run         # Show what would execute, don't run
rstep --no-merge        # Don't merge to main (for rloop orchestration)
rstep --verbose         # More output
rstep --config path.toml  # Custom config file
```

---

## Example 29: CLI Argument Parsing

**Goal:** Parse command line arguments with clap.

**What it does:**
1. Add `clap` dependency with derive feature
2. Define `Args` struct matching RSTEP.md CLI spec
3. Parse arguments and print them (no execution yet)

**Key code:**
```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "rstep")]
#[command(about = "Run one step of the rloop MDP")]
struct Args {
    /// Task ID to execute (optional, defaults to next ready task from br)
    task_id: Option<String>,

    /// Dry run - show what would execute without running
    #[arg(long)]
    dry_run: bool,

    /// Don't merge to main after success (for parallel orchestration)
    #[arg(long)]
    no_merge: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Path to config file (default: .rloop/config.toml)
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.dry_run {
        println!("DRY RUN - would execute:");
        println!("  Task: {:?}", args.task_id);
        println!("  No merge: {}", args.no_merge);
        return Ok(());
    }

    // TODO: actual execution
    Ok(())
}
```

**Commits:**
1. `deps: add clap with derive`
2. `ex-29: CLI argument parsing (N lines)`

---

## Example 30: Task Resolution

**Goal:** Resolve which task to run.

**What it does:**
1. If task_id provided on CLI, use it
2. Otherwise, get next ready task from `br ready --json --limit 1`
3. Print resolved task info

**Key code:**
```rust
fn resolve_task(args: &Args) -> Result<Task> {
    match &args.task_id {
        Some(id) => {
            // Get specific task from br
            let output = Command::new("br")
                .args(["show", id, "--json"])
                .output()?;
            serde_json::from_slice(&output.stdout)
        }
        None => {
            // Get next ready task
            let output = Command::new("br")
                .args(["ready", "--json", "--limit", "1"])
                .output()?;
            let tasks: Vec<Task> = serde_json::from_slice(&output.stdout)?;
            tasks.into_iter().next()
                .ok_or_else(|| anyhow!("No ready tasks"))
        }
    }
}
```

**Commits:**
1. `ex-30: task resolution from CLI or br (N lines)`

---

## Example 31: Dry Run Implementation

**Goal:** Full dry run that shows everything without executing.

**What it does:**
1. Load config
2. Resolve task
3. Print execution plan:
   - Task details
   - Config settings
   - Verification commands
   - What would happen on success/failure

**Output format:**
```
rstep dry run
=============

Task: bd-abc123
  Title: Fix the add function bug
  Priority: 2
  Type: bug

Config: .rloop/config.toml
  Model: claude-sonnet-4-20250514
  Max turns: 50
  Tools: Read, Write, Edit, Bash, Glob, Grep

Execution plan:
  1. Create worktree: .rloop/worktrees/bd-abc123
  2. Lock worktree
  3. Run Claude with task prompt
  4. Verify: cargo check, cargo test
  5. On success: merge to main, cleanup
  6. On failure: cleanup, leave task open

Would you like to proceed? (not implemented - this is dry run)
```

**Commits:**
1. `ex-31: dry run implementation (N lines)`

---

## Example 32: Main Binary

**Goal:** Wire up the actual step execution in main.rs.

**What it does:**
1. Replace placeholder main.rs with full CLI
2. Parse args
3. Load config (from CLI path or default)
4. Resolve task
5. Call `run_step()` from library
6. Handle result (print summary, set exit code)

**Key code:**
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Setup logging
    if args.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("rstep=debug")
            .init();
    }

    // Dry run
    if args.dry_run {
        return dry_run(&args);
    }

    // Load config
    let repo_path = std::env::current_dir()?;
    let config_path = args.config.unwrap_or_else(|| repo_path.join(".rloop/config.toml"));
    let config = load_config(&config_path);

    // Override auto_merge if --no-merge
    let config = if args.no_merge {
        Config { worktree: WorktreeConfig { auto_merge: false, ..config.worktree }, ..config }
    } else {
        config
    };

    // Resolve task
    let task = resolve_task(&args)?;

    // Execute step
    let result = run_step(&repo_path, &task, &config).await?;

    // Handle result
    match &result {
        StepResult::Success { summary, tokens_used, duration_ms } => {
            println!("SUCCESS: {}", summary);
            println!("  Tokens: {}", tokens_used);
            println!("  Duration: {}ms", duration_ms);
            std::process::exit(0);
        }
        StepResult::Failed { reason, next_steps, tokens_used } => {
            println!("FAILED: {}", reason);
            for step in next_steps {
                println!("  - {}", step);
            }
            println!("  Tokens: {}", tokens_used);
            std::process::exit(1);
        }
        StepResult::Blocked { blocker } => {
            println!("BLOCKED: {:?}", blocker);
            std::process::exit(2);
        }
    }
}
```

**Exit codes:**
- 0: Success
- 1: Failed
- 2: Blocked

**Commits:**
1. `ex-32: main binary implementation (N lines)`

---

## Example 33: Polish and Testing

**Goal:** Final polish - help text, error handling, integration tests.

**What it does:**
1. Improve clap help text and examples
2. Better error messages for common failures:
   - br not installed
   - No ready tasks
   - Config file not found
   - Git repo not found
3. Add integration test using recorded fixtures

**Help output:**
```
rstep - Run one step of the rloop MDP

Usage: rstep [OPTIONS] [TASK_ID]

Arguments:
  [TASK_ID]  Task ID to execute (default: next ready task from br)

Options:
      --dry-run         Show what would execute without running
      --no-merge        Don't merge to main after success
  -v, --verbose         Verbose output
      --config <PATH>   Path to config file [default: .rloop/config.toml]
  -h, --help            Print help
  -V, --version         Print version

Examples:
  rstep                    Run next ready task
  rstep bd-abc123          Run specific task
  rstep --dry-run          Preview without executing
  rstep --no-merge         Skip merge (for parallel orchestration)
```

**Commits:**
1. `ex-33: CLI polish and testing (N lines)`

---

## Beyond Example 33

With the CLI complete, rstep can be used standalone:

```bash
# Install
cargo install --path rstep

# Use
cd my-project
rstep              # Run next task
rstep bd-123       # Run specific task
rstep --dry-run    # Preview
```

Future work (not in this phase):
- **Retry logic**: Automatic retry with exponential backoff
- **Context management**: Smart context loading based on task type
- **Session persistence**: Resume interrupted sessions
- **Metrics export**: JSON/CSV output for analysis
- **Shell completion**: Tab completion for task IDs

---

## Testing Strategy

Each example should have:
1. The example itself (runnable demo)
2. Corresponding test using recorded fixtures

Run real API once to record, then tests replay for speed.

```bash
# Record (costs money, slow)
RECORD=1 cargo run --example 05-verification

# Test (free, fast)
cargo test test_ex_05
```
