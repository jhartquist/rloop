# rstep Examples Plan

Chronological examples building up the step executor.

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

## Later Examples (TBD)

- Worktree setup
- Merge flow
- Human queue tools
- Security/sandbox tests
- Multiple tools (ask_question, log_assumption, etc.)

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
