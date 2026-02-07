# Research: LLM-Based Verification

## Context

Today, verification in rloop is command-based. When the task agent calls `complete`, `run_verification` in `tools.rs` runs each shell command (e.g. `cargo test`, `cargo clippy`) in sequence. If any command fails, the error output is returned to the agent as `is_error: true`, triggering a retry. If no verification commands are configured, calling `complete` is sufficient for success.

This works well for objectively testable criteria: does the code compile, do tests pass. But many task outcomes are harder to verify with commands alone:

- Was the code well-structured and idiomatic?
- Does the implementation match the acceptance criteria in the task description?
- Were edge cases handled?
- Did the agent actually implement what was asked, or just something that happens to pass the tests?
- For research/writing tasks, is the output high quality and complete?

The goal: add an optional LLM verification pass that reviews the agent's work and judges whether the task was actually completed correctly. This is the second ancillary agent use case (alongside task enhancement from research 01), reinforcing the need for a shared `run_query` abstraction.

---

## 1. Approaches to LLM Verification

### Approach A: Full Review

The verifier receives the complete task description, acceptance criteria, and full access to the worktree (via Read, Glob, Grep, Bash tools). It explores the code freely and renders a pass/fail judgment.

**What the verifier sees:**
- Task description and acceptance criteria (from the task markdown body)
- Full worktree access via tools
- No diff — it reads whatever it needs

**Prompt structure:**
```
You are a code reviewer verifying that a task was completed correctly.

## Task Description
{task.description}

## Instructions
Review the code in this worktree and determine whether the task was completed
correctly according to the acceptance criteria.

Use Read, Glob, Grep, and Bash to explore the codebase. You may run tests
or build commands to verify correctness.

## Output
Respond with a JSON object:
{
  "pass": true/false,
  "reasoning": "explanation of your judgment",
  "issues": ["list of specific issues found, if any"]
}
```

**Trade-offs:**
- (+) Most thorough review — verifier can explore anything
- (+) Catches issues that commands miss (wrong approach, missing edge cases, poor structure)
- (+) Can verify writing/research tasks with no testable command
- (-) Most expensive — verifier may use many tool calls, consuming significant tokens
- (-) Slowest — tool calls add latency on top of LLM inference
- (-) Hardest to control cost — token usage varies based on codebase size and verifier curiosity
- (-) Risk of false negatives — verifier may misunderstand the codebase and fail valid work

**Best for:** High-value tasks, Opus-level work, tasks with complex acceptance criteria that aren't easily tested by commands.

### Approach B: Diff-Only Review

The verifier receives the task description plus a `git diff` of all changes made by the agent. No tool access — it judges purely from the diff.

**What the verifier sees:**
- Task description and acceptance criteria
- `git diff` output (all changes made by the agent)
- No tool access — read-only, single-turn

**How to generate the diff:**
```rust
// In the worktree, diff against the base branch
let diff = git::git(&worktree.path, &["diff", &base_branch, "HEAD"])?;
```

**Prompt structure:**
```
You are a code reviewer. A coding agent was given the following task and
produced the changes shown in the diff below.

## Task
{task.description}

## Changes (git diff)
{diff}

## Instructions
Review whether the changes correctly and completely implement the task.
Check for: correctness, completeness vs acceptance criteria, code quality,
edge cases, and potential issues.

Respond with JSON:
{
  "pass": true/false,
  "reasoning": "explanation",
  "issues": []
}
```

**Trade-offs:**
- (+) Predictable cost — token count is proportional to diff size, not codebase size
- (+) Fast — single LLM call with no tool use, typically 5-15 seconds
- (+) Simple implementation — no tool setup, just prompt → response
- (+) Natural fit for code review — humans review diffs too
- (-) Missing context — can't see files that weren't changed but are relevant
- (-) Can't verify runtime behavior — only sees static changes
- (-) Large diffs may exceed useful context (multi-file refactors, new modules)
- (-) Can't catch what's missing — only sees what was changed, not what should have been

**Best for:** Most code tasks where the diff captures the essential work. Fast, cheap, good default.

### Approach C: Checklist-Based Review

The verifier receives the acceptance criteria extracted from the task description as a structured checklist and verifies each item individually. Can be combined with either full review or diff-only.

**Prompt structure:**
```
You are verifying a completed task against its acceptance criteria.

## Acceptance Criteria
1. Reads .wav files via `hound` crate
2. Frames at 1024 samples with 512 hop
3. FFT via `rustfft`, output is magnitude spectrum
4. Unit tests cover mono 16-bit and stereo conversion

## Changes (git diff)
{diff}

## Instructions
For each acceptance criterion, determine whether it is met by the changes.

Respond with JSON:
{
  "pass": true/false,
  "criteria": [
    {"criterion": "Reads .wav files via hound crate", "met": true, "evidence": "..."},
    {"criterion": "Frames at 1024 samples", "met": true, "evidence": "..."},
    ...
  ],
  "overall_reasoning": "..."
}
```

**Trade-offs:**
- (+) Most structured and auditable — clear per-criterion verdicts
- (+) Easy to understand why verification passed or failed
- (+) Reduces false negatives — each criterion is judged independently
- (+) Naturally maps to task file format (acceptance criteria section)
- (-) Requires well-structured acceptance criteria in the task file
- (-) May miss issues not covered by the criteria (code quality, edge cases)
- (-) Slightly more complex prompt engineering
- (-) Needs parsing logic to extract criteria from markdown

**Best for:** Tasks with clear, enumerated acceptance criteria. Excellent audit trail.

### Approach D: Multi-Model Verification

Use a different (potentially stronger) model for verification than the task agent. For example, the task runs on Sonnet while verification runs on Opus.

This is orthogonal to the approaches above — it's a model selection strategy that applies to any of A/B/C.

**Configuration:**
```toml
# .rloop/config.toml
[step]
model = "Sonnet"

[verification]
llm = true
llm_model = "Opus"           # use stronger model for verification
llm_approach = "diff"         # or "full", "checklist"
```

Per-task override:
```yaml
---
id: 5
model: Sonnet
llm_verification: true
llm_verification_model: Opus
---
```

**Trade-offs:**
- (+) Stronger model may catch issues the task model missed
- (+) Cost can still be controlled (Opus for verification is cheaper than Opus for task execution because the context is smaller)
- (+) Natural upgrade path — verify with Sonnet today, Opus for critical tasks
- (-) Adds complexity to model configuration
- (-) Counter-intuitive: if Opus is better, why not use it for the task?
- (-) Verification cost scales with model price

**Best for:** Critical tasks where the extra cost of a stronger verifier is justified. Also useful for validating work done by cheaper models.

---

## 2. Integration with Existing Verification

### Relationship Between Command and LLM Verification

Three options for how LLM verification relates to command verification:

**Option 1: LLM supplements commands (recommended)**

Commands run first. If commands pass, LLM verification runs as a second gate. Both must pass for the task to succeed.

```
Agent calls complete
  → Run command verification (cargo test, etc.)
    → Commands fail? → Return error to agent (retry)
    → Commands pass? → Run LLM verification
      → LLM fails? → Return LLM feedback to agent (retry)
      → LLM passes? → Task succeeds
```

Rationale: Commands are fast, deterministic, and cheap. Run them first to catch obvious failures before spending tokens on LLM review. LLM verification catches the semantic issues that commands miss.

**Option 2: LLM replaces commands**

For tasks with no natural command-based verification (research, writing, design), LLM verification is the only check.

```
Agent calls complete
  → No command verification configured
  → Run LLM verification
    → LLM fails? → Return feedback to agent (retry)
    → LLM passes? → Task succeeds
```

This is the natural mode for non-code tasks. Today these tasks succeed immediately on `complete` — LLM verification adds quality control.

**Option 3: LLM only (skip commands even if configured)**

Use LLM as the sole verifier, ignoring command verification. Not recommended — commands provide deterministic signal that LLM review can't replace.

### Recommended Default

**Option 1 for tasks with commands, Option 2 for tasks without.** The configuration should be:

```toml
# .rloop/config.toml
[verification]
llm = false                  # opt-in globally
llm_model = "Sonnet"         # default verification model
llm_approach = "diff"        # default approach
```

Per-task frontmatter:
```yaml
---
id: 5
verification: "cargo test"
llm_verification: true       # enable LLM review after commands pass
---
```

For tasks with no `verification` field but `llm_verification: true`, LLM is the sole check.

### Where LLM Verification Happens in the Code

Today the verification flow lives in `build_complete_tool` in `tools.rs`:

```rust
// Current flow in the complete tool closure:
if let Some(ref v) = cmds
    && let Err(msg) = run_verification(&v.0, &dir, cmd_timeout)
{
    return Ok(tool_err(msg));  // triggers retry
}

// Verification passed — accept completion
*summary_ref.lock().expect("...") = Some(text);
stop_ref.store(true, Ordering::SeqCst);
Ok(tool_ok("Task complete."))
```

LLM verification inserts between command verification passing and accepting completion:

```rust
// After command verification passes:
if llm_verification_enabled {
    let llm_result = run_llm_verification(&task_description, &dir, &llm_config).await;
    if !llm_result.pass {
        return Ok(tool_err(format!(
            "LLM verification failed:\n{}\n\nIssues:\n{}",
            llm_result.reasoning,
            llm_result.issues.join("\n- ")
        )));
    }
}
```

**Key complication:** The `complete` tool closure is synchronous in the current design (spawned via `tool!` macro). LLM verification requires an async call to `run_query`. Options:

1. **Make the tool closure async-aware** — Use `tokio::runtime::Handle::current().block_on()` inside the closure. This works but blocks the tokio thread.

2. **Move LLM verification out of the tool** — Run it after the step completes, in `session.rs`. If LLM verification fails, the step is retried with feedback. This is cleaner architecturally but changes the retry semantics (a failed LLM verification becomes a full step retry rather than a within-step retry).

3. **Two-phase complete** — The `complete` tool runs command verification synchronously. If it passes, the tool returns success and the stream is interrupted. Then `execute_step` runs LLM verification before returning `StepOutcome::Success`. If LLM verification fails, it returns `StepOutcome::Failed` with the LLM feedback, and the session retry loop handles it.

**Recommended: Option 3 (two-phase complete).** It keeps the `complete` tool simple and synchronous, puts the async LLM call in `execute_step` where it belongs, and uses the existing retry loop in `run_with_retries` for LLM failures. The flow becomes:

```
Agent calls complete(summary)
  → complete tool runs command verification (sync)
    → Commands fail? → Return error, agent retries (within-step)
    → Commands pass? → Return success, interrupt stream

execute_step continues:
  → LLM verification enabled?
    → Run run_query with verification prompt (async)
      → LLM passes? → Return StepOutcome::Success
      → LLM fails? → Return StepOutcome::Failed { learnings: llm_feedback }
        → run_with_retries handles retry with LLM feedback in prior_learnings
```

This means LLM verification failure triggers a full step retry (new agent session), not a within-step retry. This is actually better:
- The agent gets a fresh context window
- The LLM feedback is included in `prior_learnings`
- No risk of the agent's context being too full to act on the feedback
- Simpler implementation — no need to inject async into the tool closure

### Changes to `execute_step`

```rust
pub async fn execute_step(...) -> anyhow::Result<StepResult> {
    // ... existing steps 1-9 ...

    // 10. LLM verification (if enabled and command verification passed)
    let outcome = determine_outcome(&summary, data.result_message.as_ref());

    let outcome = if let StepOutcome::Success { ref summary } = outcome
        && let Some(llm_config) = &config.llm_verification
    {
        let llm_result = run_llm_verification(
            task, worktree_path, summary, llm_config
        ).await?;

        if llm_result.pass {
            outcome  // keep Success
        } else {
            StepOutcome::Failed {
                reason: format!("LLM verification failed: {}", llm_result.reasoning),
                learnings: llm_result.issues.join("; "),
            }
        }
    } else {
        outcome
    };

    Ok(StepResult { outcome, ..rest })
}
```

---

## 3. Verification Agent Design

### What the Verifier Sees

Regardless of approach (full, diff, checklist), the verifier always receives:

1. **Task description** — The full markdown body from the task file, including acceptance criteria
2. **Agent's completion summary** — The summary string passed to `complete`

For diff-only review, additionally:
3. **Git diff** — Output of `git diff {base}..HEAD` in the worktree

For full review, additionally:
3. **Worktree access** — Read, Glob, Grep, Bash tools scoped to the worktree

### Tool Access

| Tool | Full Review | Diff-Only | Checklist |
|------|-------------|-----------|-----------|
| Read | Yes | No | Optional |
| Glob | Yes | No | No |
| Grep | Yes | No | No |
| Bash | Yes (read-only intent) | No | Optional |
| Write | **No** | No | No |
| Edit | **No** | No | No |
| WebFetch | No | No | No |
| WebSearch | No | No | No |

The verifier should never have Write or Edit access. It observes, it doesn't modify. Bash access in full review is for running build/test commands and inspecting output, not for modifying files. (The worktree will be merged or cleaned up regardless.)

### Structured Output

The verifier must return structured output for programmatic parsing. Use the SDK's `output_format` to enforce a JSON schema:

```rust
#[derive(Deserialize, JsonSchema)]
pub struct LlmVerificationResult {
    pub pass: bool,
    pub reasoning: String,
    pub issues: Vec<String>,
}
```

For checklist-based verification:
```rust
#[derive(Deserialize, JsonSchema)]
pub struct ChecklistResult {
    pub pass: bool,
    pub criteria: Vec<CriterionResult>,
    pub overall_reasoning: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CriterionResult {
    pub criterion: String,
    pub met: bool,
    pub evidence: String,
}
```

**Note:** This is the first ancillary agent use case that needs structured output from `run_query`. Task enhancement (research 01) returns plain text. This means `QueryConfig` may need an optional `output_format` field, or LLM verification could use a separate code path. See section 4 for the shared infrastructure analysis.

### Interaction Model

The verifier does **not** interact with the task agent. It is a separate, independent assessment. The flow is:

1. Task agent finishes and calls `complete`
2. Command verification passes
3. Stream is interrupted, `execute_step` has the outcome
4. LLM verifier runs independently in the same worktree
5. Verifier renders judgment
6. If fail: `execute_step` returns `StepOutcome::Failed`; retry loop re-runs task agent with feedback
7. Task agent never talks to verifier directly

This is intentional — the verifier is an independent judge, not a pair programmer. The task agent's only feedback from the verifier comes indirectly via `prior_learnings` on retry.

### Same Worktree or Separate?

The verifier should run in the **same worktree** as the task agent. Reasons:
- The worktree contains the agent's changes — that's what we're verifying
- No need to copy files or create a second worktree
- The verifier has Read-only tools, so it won't modify anything
- The worktree hasn't been merged yet, so there's no risk of interference

For full review mode, the verifier's `cwd` is set to the worktree path, and it gets project settings loaded (`setting_sources: vec![SettingSource::Project]`) so it can read CLAUDE.md and understand project conventions.

---

## 4. Shared Query Infrastructure

### The Ancillary Agent Pattern (Updated)

Research 01 identified three features needing a lightweight LLM query path:

| Feature | Model | Tools | Structured Output | CWD |
|---------|-------|-------|-------------------|-----|
| Task Enhancement | Sonnet | none | none | repo root |
| LLM Verification (diff) | Sonnet/Opus | none | yes (pass/fail) | worktree |
| LLM Verification (full) | Sonnet/Opus | Read, Glob, Grep, Bash | yes (pass/fail) | worktree |
| HumanQ Answer Processing | Sonnet | none | none | repo root |

LLM verification introduces two new requirements for `run_query` that task enhancement didn't need:

1. **Structured output** — The verifier must return JSON matching a schema
2. **Setting sources** — Full review mode needs project settings loaded

### Updated `QueryConfig`

```rust
pub struct QueryConfig {
    pub model: Model,
    pub cwd: PathBuf,
    pub tools: Vec<BuiltinTool>,
    pub setting_sources: Vec<SettingSource>,
    pub output_format: Option<serde_json::Value>,    // NEW: for structured output
}
```

The `output_format` field is optional. When `Some`, the SDK enforces a JSON response schema. When `None`, the response is plain text (task enhancement, HumanQ).

### Do We Need `run_query` or Something More?

For diff-only verification, `run_query` is perfect: single prompt in, structured JSON out, no tools, fast.

For full-review verification with tools, `run_query` still works but the interaction may be multi-turn (agent reads files, runs commands, then produces its verdict). The rstep v1 `run_query` already handles multi-turn tool use — it streams all messages and returns the final result. The key difference from `execute_step` is that there's no MCP tools, no interruption, and no retry within the query.

**Recommendation:** `run_query` handles both cases. The structured output forces the final message to be the JSON verdict regardless of how many tool calls precede it. The `on_message` callback (from rstep v1's `run_query_with`) can be used to log verifier events to the session log.

### Updated `run_query` Signature

```rust
pub struct QueryResult {
    pub response: String,                            // text response
    pub structured_output: Option<serde_json::Value>, // if output_format was set
    pub cost_usd: f64,
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub async fn run_query(prompt: &str, config: &QueryConfig) -> Result<QueryResult>;
```

### Usage for LLM Verification

```rust
// Diff-only verification
let diff = git::git(&worktree_path, &["diff", &base_branch, "HEAD"])?;
let prompt = build_verification_prompt(&task.description, &summary, &diff);
let schema = schemars::schema_for!(LlmVerificationResult);

let result = run_query(&prompt, &QueryConfig {
    model: config.llm_verification.model,
    cwd: worktree_path.to_path_buf(),
    tools: vec![],
    setting_sources: vec![],
    output_format: Some(json!({ "type": "json_schema", "schema": schema })),
}).await?;

let verdict: LlmVerificationResult =
    serde_json::from_value(result.structured_output.unwrap())?;
```

```rust
// Full review verification
let prompt = build_full_review_prompt(&task.description, &summary);

let result = run_query(&prompt, &QueryConfig {
    model: config.llm_verification.model,
    cwd: worktree_path.to_path_buf(),
    tools: vec![BuiltinTool::Read, BuiltinTool::Glob, BuiltinTool::Grep, BuiltinTool::Bash],
    setting_sources: vec![SettingSource::Project],
    output_format: Some(json!({ "type": "json_schema", "schema": schema })),
}).await?;
```

---

## 5. Cost and Latency

### Baseline Costs

LLM verification adds tokens to every verified task. Estimated costs per approach:

| Approach | Input Tokens | Output Tokens | Sonnet Cost | Opus Cost | Latency |
|----------|-------------|---------------|-------------|-----------|---------|
| Diff-only (small diff) | ~2k task + ~3k diff = 5k | ~500 | ~$0.02 | ~$0.08 | 3-8s |
| Diff-only (large diff) | ~2k task + ~20k diff = 22k | ~500 | ~$0.07 | ~$0.34 | 5-15s |
| Full review (simple) | ~2k task + ~10k files read = 12k | ~1k + tool calls | ~$0.05 | ~$0.20 | 15-45s |
| Full review (complex) | ~2k task + ~50k files = 52k | ~2k + tool calls | ~$0.17 | ~$0.80 | 30-90s |
| Checklist (5 criteria) | ~3k task + ~3k diff = 6k | ~1k | ~$0.03 | ~$0.10 | 5-10s |

For context, a typical task execution costs $0.20-2.00 on Sonnet and $1-8 on Opus. Diff-only verification adds 1-10% to the total cost. Full review can add 5-40%.

### Strategies to Minimize Cost

**1. Default to diff-only**

Diff-only is the sweet spot for most tasks. It captures the essential changes, costs pennies, and runs in seconds. Reserve full review for high-value tasks explicitly.

```toml
[verification]
llm_approach = "diff"  # cheap default
```

**2. Use a cheaper model for verification**

Sonnet is sufficient for most verification. The verifier doesn't need to write code — it judges existing code. Sonnet's reasoning capability is adequate for diff review and checklist verification.

```toml
[verification]
llm_model = "Sonnet"  # even when task runs on Opus
```

**3. Truncate large diffs**

For diff-only review, cap the diff at a reasonable size (e.g., 20k tokens). If the diff exceeds this, truncate with a note:

```
[Diff truncated — showing first 20,000 tokens of {total_tokens} total.
Focus your review on the changes shown.]
```

**4. Skip verification for trivial tasks**

Tasks marked with specific tags or below a complexity threshold can skip LLM verification:

```yaml
---
id: 5
llm_verification: false  # simple refactor, commands are enough
verification: "cargo test"
---
```

**5. Cache verifier prompts**

The task description portion of the verification prompt is identical across retries. Claude's prompt caching will automatically cache this prefix, reducing cost on retries.

### Reporting Verification Cost

Add verification cost tracking to `StepResult` and `SessionMetrics`:

```rust
pub struct StepResult {
    // ... existing fields ...
    pub verification_cost_usd: f64,
    pub verification_duration: Duration,
    pub verification_input_tokens: u64,
    pub verification_output_tokens: u64,
}
```

New event type for session logging:

```rust
Event::LlmVerificationRan {
    task_id: TaskId,
    approach: String,          // "diff", "full", "checklist"
    model: String,
    pass: bool,
    reasoning: String,
    cost_usd: f64,
    duration: Duration,
    input_tokens: u64,
    output_tokens: u64,
}
```

Terminal output:
```
  ✓ LLM verify task 05 (diff, Sonnet): pass  $0.02  3.2s
  ✗ LLM verify task 05 (full, Opus): fail  $0.34  12.1s
    Issues:
    - Missing error handling for empty input files
    - Unit test doesn't cover stereo conversion
```

### Making Verification Optional

Verification should be configurable at three levels:

1. **Project-wide default** in `.rloop/config.toml`:
   ```toml
   [verification]
   llm = false                # off by default
   llm_model = "Sonnet"
   llm_approach = "diff"
   ```

2. **Per-task override** in task frontmatter:
   ```yaml
   llm_verification: true
   llm_verification_model: Opus
   llm_verification_approach: full
   ```

3. **CLI override** (future):
   ```bash
   rloop run 05 --llm-verify         # force LLM verification
   rloop run 05 --no-llm-verify      # skip LLM verification
   ```

Resolution order: CLI > task frontmatter > project config > default (off).

---

## 6. Configuration Design

### Task Frontmatter Extensions

```yaml
---
id: 5
model: Sonnet
verification: "cargo test"
llm_verification: true                    # enable LLM review
llm_verification_model: Opus              # optional: override model
llm_verification_approach: checklist      # optional: override approach
completed: false
---
```

All `llm_verification_*` fields are optional. When `llm_verification: true` is set without model/approach, the project defaults apply.

### Config Extensions

```rust
pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
    pub timeout_seconds: u64,
    pub llm_verification: Option<LlmVerificationConfig>,  // NEW
}

pub struct LlmVerificationConfig {
    pub model: Model,
    pub approach: VerificationApproach,
    pub max_diff_tokens: usize,           // truncation threshold for diff-only
}

pub enum VerificationApproach {
    Diff,
    Full,
    Checklist,
}
```

### TOML Format

```toml
[step]
model = "Opus"
max_turns = 50
max_retries = 10

[verification]
llm = true
llm_model = "Sonnet"
llm_approach = "diff"
max_diff_tokens = 20000
```

---

## 7. Trade-off Summary

| Approach | Accuracy | Cost | Latency | Complexity | Best For |
|----------|----------|------|---------|------------|----------|
| Diff-only | Good | Low ($0.02-0.07) | Low (3-15s) | Low | Most code tasks |
| Full review | High | Medium-High ($0.05-0.80) | Medium-High (15-90s) | Medium | Complex or critical tasks |
| Checklist | High (structured) | Low ($0.03-0.10) | Low (5-10s) | Medium | Tasks with clear acceptance criteria |
| Multi-model | Highest | Highest | Same as approach | Low (config only) | Critical tasks worth extra cost |
| Commands only (status quo) | For tests: high; for quality: none | Zero | Zero | Zero | Tasks with good test coverage |

### Recommended v1 Scope

1. **Diff-only as default approach** — good enough for most tasks, cheap, fast
2. **Sonnet as default verification model** — sufficient for judging diffs, cost-effective
3. **Two-phase complete** — commands first (sync in tool), LLM second (async in `execute_step`)
4. **Optional per-task and per-project** — `llm_verification: true` in frontmatter or config
5. **Shared `run_query` with `output_format`** — extend the query abstraction from research 01
6. **Structured output** — `LlmVerificationResult { pass, reasoning, issues }`
7. **Cost tracking** — separate verification cost in `StepResult` and session metrics

Defer to v2:
- Full review mode (tools for the verifier)
- Checklist-based review (requires acceptance criteria extraction)
- Multi-model verification
- CLI overrides (`--llm-verify`, `--no-llm-verify`)

---

## 8. Relationship to the Shared Ancillary Agent Pattern

Research 01 proposed `rloop::query::run_query` as a shared lightweight LLM query path. LLM verification is the second consumer of this pattern, and it introduces a new requirement: structured output.

### What Changes from Research 01

Research 01's `QueryConfig`:
```rust
pub struct QueryConfig {
    pub model: Model,
    pub cwd: PathBuf,
    pub tools: Vec<BuiltinTool>,
    pub setting_sources: Vec<SettingSource>,
}
```

LLM verification needs:
```rust
pub struct QueryConfig {
    pub model: Model,
    pub cwd: PathBuf,
    pub tools: Vec<BuiltinTool>,
    pub setting_sources: Vec<SettingSource>,
    pub output_format: Option<serde_json::Value>,   // NEW
}
```

The addition is minimal — one optional field. The `run_query` implementation checks `output_format` and passes it to `ClaudeAgentOptions::builder().output_format(...)` if present. The `QueryResult` gains a `structured_output: Option<serde_json::Value>` field extracted from the `Result` message.

### Three Consumers, One Abstraction

```rust
// Task enhancement (research 01)
run_query(&prompt, &QueryConfig {
    model: Model::Sonnet,
    cwd: repo_root,
    tools: vec![],
    setting_sources: vec![],
    output_format: None,
})

// LLM verification — diff-only (this research)
run_query(&prompt, &QueryConfig {
    model: Model::Sonnet,
    cwd: worktree_path,
    tools: vec![],
    setting_sources: vec![],
    output_format: Some(verification_schema),
})

// LLM verification — full review (future)
run_query(&prompt, &QueryConfig {
    model: Model::Opus,
    cwd: worktree_path,
    tools: vec![Read, Glob, Grep, Bash],
    setting_sources: vec![SettingSource::Project],
    output_format: Some(verification_schema),
})

// HumanQ answer processing (future)
run_query(&prompt, &QueryConfig {
    model: Model::Sonnet,
    cwd: repo_root,
    tools: vec![],
    setting_sources: vec![],
    output_format: None,
})
```

The pattern holds. `run_query` with `QueryConfig` serves all four cases with the addition of one optional field.

### Implementation Priority

1. **First:** Implement `run_query` with the full `QueryConfig` (including `output_format`)
2. **Second:** Use it for task enhancement (`--enhance` on `rloop task new`)
3. **Third:** Use it for LLM verification (diff-only mode, opt-in per task)
4. **Fourth:** Extend to full review mode and checklist mode as needed

Steps 1-2 are research 01's scope. Steps 3-4 are this research's scope. The shared infrastructure makes both cheaper to build.

---

## 9. Open Questions

### Must Resolve Before Implementation

1. **When to use LLM vs command verification?** Recommendation: LLM verification is always opt-in. Commands are the primary verification mechanism. LLM is a supplement for tasks where commands aren't sufficient (writing tasks, quality review) or where higher assurance is needed. Default: off.

2. **Who decides pass/fail if commands pass but LLM fails?** The LLM verdict takes precedence. If commands pass but LLM fails, the task fails. Rationale: the whole point of adding LLM verification is to catch things commands miss. If we ignore the LLM's judgment, there's no point running it. The feedback is passed to the retry loop.

3. **How to handle false negatives from the verifier?** False negatives (verifier rejects valid work) are the biggest risk. Mitigations:
   - Max LLM verification retries (e.g., 2) — if the verifier rejects twice, accept the task. This prevents infinite loops from a confused verifier.
   - The retry limit for verification should be separate from the main task retry limit, and much smaller.
   - Log all verification verdicts so humans can review false negative rates.
   - Consider a `--force-complete` flag or similar escape hatch for when the verifier is wrong.

4. **Should verification retries count against `max_retries`?** Yes — each LLM verification failure triggers a full step retry, which counts against the task's `max_retries`. This is consistent: from the session's perspective, a failed verification is a failed step.

5. **What if the diff is empty?** If the agent called `complete` but made no changes, the diff is empty. The verifier should still run — the task might be about verifying existing behavior, creating documentation, etc. But for a typical code task, an empty diff is suspicious and the verifier should flag it.

### Worth Discussing

6. **Should the verifier see prior learnings?** No — the verifier judges the current state of the code, not the agent's journey. Prior learnings are for the task agent only.

7. **Should the verifier see the agent's completion summary?** Yes — the summary provides context about what the agent intended to do, which helps the verifier assess whether the intent matches the result.

8. **Can the verifier provide specific fix instructions?** Yes, via the `issues` field in `LlmVerificationResult`. These are passed as `prior_learnings` to the retry, giving the task agent specific guidance. This is one of the most valuable aspects of LLM verification — it doesn't just fail, it says why and what to fix.

9. **Should LLM verification be skippable mid-session?** If a task fails LLM verification twice and the human is watching, they might want to accept it anyway. This requires interactive intervention, which rloop doesn't support in v1. For now, the escape hatch is to set `llm_verification: false` on the task and re-run.

10. **How does this interact with parallel DAG execution?** LLM verification runs after the agent completes but before the worktree is merged. For parallel execution (research 02), this means the verifier runs in the task's worktree, which is independent of other tasks. No special handling needed — verification is per-task and fully isolated.
