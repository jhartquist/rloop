# Research: LLM-Powered Task Enhancement

## Context

`rloop task new` currently accepts a description string and optional flags (title, model, depends-on, verification), generates frontmatter, writes the file, and commits. The idea: add an optional LLM pass that rewrites a rough description into a well-structured task with a proper title, acceptance criteria, and clean markdown.

This is also the first of several "ancillary agent" features (task enhancement, LLM verification, future HumanQ answer processing) that all need a lightweight LLM query path.

---

## 1. Agent Query Architecture

### Current State

The step executor in `step.rs` is heavy machinery:
- Creates MCP tools (complete, context_usage) with shared state (Arc<Mutex>, AtomicBool)
- Configures allowed/disallowed tool lists
- Sets structured output schema for failure fallback
- Streams responses with full event collection, token tracking, interruption handling
- Produces a `StepResult` with metrics, events, outcome

Task enhancement needs almost none of this. No MCP tools, no structured output fallback, no verification, no retry. It just needs: prompt in, text out.

### Precedent: rstep v1 `query.rs`

The archive has exactly this pattern. `run_query()` in `archive/rstep-era/rstep_v1/src/query.rs` provides:

```rust
pub async fn run_query(
    prompt: &str,
    model: Model,
    project_dir: impl AsRef<Path>,
    tools: &[BuiltinTool],
) -> Result<QueryResult>
```

It connects, queries, streams to completion, and returns the text response plus cost/duration. Clean, simple, reusable.

### Approaches

**A. Port `run_query` into rloop as `query.rs`**

Add a new module `rloop::query` with a similar signature:

```rust
pub struct QueryResult {
    pub response: String,
    pub cost_usd: f64,
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub async fn run_query(
    prompt: &str,
    model: Model,
    cwd: &Path,
    tools: &[BuiltinTool],
) -> Result<QueryResult>;
```

This shares the SDK dependency and `Model`/`BuiltinTool` types with step.rs but has its own simpler stream processing. No MCP tools, no structured output, no interruption. Setting sources set to `vec![]` (no project settings loaded — the enhancement prompt is self-contained).

Trade-offs:
- (+) Clean separation from step.rs complexity
- (+) Directly reusable for LLM verification (task 18), HumanQ answer processing (task 19), and any future single-query features
- (+) Proven pattern from rstep v1
- (-) Some code duplication with step.rs stream processing (but the simplification is worth it)

**B. Parameterize `execute_step` to handle both cases**

Add options to skip MCP tools, structured output, etc. Make `execute_step` flexible enough to serve as both the full step executor and the lightweight query path.

Trade-offs:
- (+) No new module
- (-) Adds complexity to an already complex function
- (-) The two use cases are fundamentally different — a step is a multi-turn agent session with tools and verification; a query is a single-turn text-in/text-out
- (-) Couples task enhancement changes to the critical step execution path

**C. Use Claude SDK directly (inline in main.rs)**

Just call `ClaudeClient` directly in `cmd_task_new` without any shared abstraction.

Trade-offs:
- (+) Fastest to implement
- (-) No reuse for LLM verification, HumanQ, etc.
- (-) Duplicates boilerplate (connect, stream, disconnect, extract response)

### Recommendation

**Approach A** — port `run_query` into `rloop::query`. It's proven, simple, and will be reused at least three times (task enhancement, LLM verification, HumanQ answer processing). The rstep v1 implementation is ~150 lines and works.

Key design decisions:
- `setting_sources: vec![]` — enhancement prompt is self-contained, no project CLAUDE.md needed
- `tools: &[]` — no tools for task enhancement (pure text rewrite). LLM verification will pass `[Read, Glob, Grep, Bash]`
- No MCP tools, no structured output — just text response
- Returns `QueryResult` with cost/duration for terminal display

### Tools Configuration for Enhancement

Task enhancement needs **zero tools**. The LLM receives the raw description and outputs enhanced markdown. No file reading, no codebase exploration. The prompt + existing task corpus (if included) provides all context.

For LLM verification (task 18), the same `run_query` would be called with `tools: &[Read, Glob, Grep, Bash]` and `cwd` set to the worktree — completely different tool set, same query infrastructure.

---

## 2. UX and CLI Design

### Approaches

**A. `--enhance` flag on `rloop task new` (opt-in per invocation)**

```bash
rloop task new "add audio preprocessing" --enhance
rloop task new "add audio preprocessing" --enhance --model sonnet
```

The `--enhance` flag triggers an LLM pass before writing the file. The enhancement model can default to Sonnet (fast, cheap) or be overridden with `--model`. Without `--enhance`, behavior is unchanged.

Trade-offs:
- (+) Explicit, no surprises
- (+) Zero cost when not used
- (+) User controls when to spend tokens
- (-) Extra flag to remember

**B. Config default with opt-out (`--no-enhance`)**

```toml
# .rloop/config.toml
[task]
enhance = true
enhance_model = "Sonnet"
```

```bash
rloop task new "add audio preprocessing"              # enhanced by default
rloop task new "add audio preprocessing" --no-enhance  # skip enhancement
```

Trade-offs:
- (+) Once configured, every task gets enhanced automatically
- (+) Team-wide consistency
- (-) Surprising cost if someone doesn't know it's on
- (-) Slower task creation by default (API call adds 5-15s)

**C. Separate `rloop task refine` command**

```bash
rloop task new "add audio preprocessing"    # creates raw task
rloop task refine 05                        # enhances existing task 05
```

Trade-offs:
- (+) Clean separation of concerns
- (+) Can refine existing tasks, not just new ones
- (+) No flags needed on `new`
- (-) Two-step workflow (create then refine)
- (-) More commands to learn

**D. Interactive draft-and-confirm**

```bash
rloop task new "add audio preprocessing" --enhance
# ... LLM runs ...
# Shows before/after diff
# Prompts: [a]ccept, [e]dit, [r]eject?
```

Trade-offs:
- (+) User sees and approves the enhancement before commit
- (+) Catches bad rewrites
- (-) Adds interactive prompting (rloop is currently non-interactive except for Ctrl+C)
- (-) More complex implementation

### Recommended UX

**Approach A + C combined:**

1. `--enhance` on `rloop task new` for new tasks
2. `rloop task refine <id>` for enhancing existing tasks
3. Config default for whether `--enhance` is on by default:

```toml
[task]
auto_enhance = false  # opt-in by default
enhance_model = "Sonnet"
```

When `auto_enhance = true`, `rloop task new` always enhances unless `--no-enhance` is passed. When false (default), use `--enhance` explicitly.

**Output behavior:**

The enhancement is **blocking inline**. When `--enhance` is used:

1. Print: `Enhancing task description...`
2. Run LLM query (3-10 seconds with Sonnet)
3. Print: `Enhanced task 05 at .rloop/tasks/05.md ($0.002, 3.2s)`
4. Write enhanced content and commit

No interactive confirmation for v1. The user can review via `rloop task show 05` or `git diff` if they want. Interactive draft-and-confirm is a nice future enhancement but adds complexity that isn't needed yet.

### Title Generation

When `--enhance` is used and no `--title` is provided, the LLM should also generate a title. This is natural — the LLM is already reading the description and can extract a concise title as part of the rewrite.

When `--title` is explicitly provided, the LLM should use it as-is (respect user intent).

### Flag Naming

`--enhance` is the best option:
- `--ai` is too generic and vague
- `--refine` conflicts with the `rloop task refine` subcommand
- `--rewrite` implies the user's input is bad
- `--enhance` is neutral and descriptive

---

## 3. Prompt Engineering

### System Prompt Design

The enhancement prompt should be direct and structural. The LLM receives the raw description and outputs a complete task markdown body (everything below the `---` frontmatter delimiter).

```
You are a task specification writer for rloop, a DAG-based task executor that
runs coding agents in isolated git worktrees.

Given a rough task description, rewrite it into a well-structured task file
body. Your output will be placed directly below the YAML frontmatter in a
markdown file.

## Output Format

Return ONLY the markdown body. Do not include frontmatter (---/id/model/etc).
Structure:

1. A level-1 heading (# Title) — concise, descriptive
2. A 1-2 sentence summary of what the task does
3. "## Current State" section (if the description implies existing code)
4. "## What to do" section with numbered steps, each with a descriptive
   sub-heading and specific instructions
5. "## Acceptance Criteria" section with a bulleted list of testable conditions

## Guidelines

- Be specific. Reference file paths, function names, and types when the
  description mentions them.
- Keep code examples minimal — just enough to show the shape, not full
  implementations.
- If the description is already well-structured, improve it only where needed.
  Do not rewrite for the sake of rewriting.
- Do not invent requirements. Expand on what was given, but don't add features
  the user didn't ask for.
- Use present tense, imperative mood ("Add...", "Implement...", "Update...").
```

### Few-Shot Examples

Including existing task files as examples is powerful but expensive. Options:

**A. Embed 2-3 curated examples in the prompt**

Pick the best-structured tasks from the corpus (e.g., tasks 08, 10, 15) and include them as few-shot examples. This adds ~2-3k tokens to the prompt but dramatically improves output consistency.

Trade-offs:
- (+) Consistent output style matching the project's conventions
- (+) The LLM sees exactly what "good" looks like
- (-) Hardcoded examples go stale as conventions evolve
- (-) Adds prompt size (cost)

**B. Dynamically load N most recent tasks**

Read the last 3-5 tasks from `.rloop/tasks/` and include them as examples.

Trade-offs:
- (+) Always up-to-date with current conventions
- (+) No hardcoded examples to maintain
- (-) Recent tasks might not be the best examples
- (-) Adds I/O and variable prompt size
- (-) Early projects have few/no examples

**C. No examples, rely on instructions only**

Trade-offs:
- (+) Simplest, cheapest
- (-) Less consistent output
- (-) May not match project style

### Recommendation

**Approach A for v1** — embed 2-3 curated examples in the system prompt. The task corpus is small enough that hand-picking good examples is easy. As the corpus grows, consider switching to dynamic loading.

The examples should be the "ideal" task format — one simple task and one complex task with dependencies and verification.

### Handling "Good Enough" Descriptions

The prompt instruction "If the description is already well-structured, improve it only where needed" handles this. The LLM should recognize when input already has clear headings, acceptance criteria, etc. and make minimal changes.

An alternative is to add a "quality gate" — have the LLM first assess whether enhancement is needed and skip the rewrite if the input scores above a threshold. This adds complexity and a second query for marginal benefit. Not worth it for v1.

### Structured Output for Title Extraction

When the LLM generates both a title and a body, we need to parse them apart. Options:

**A. Convention-based parsing**: The first `# Heading` in the output becomes the title. The rest is the body.

**B. Structured output**: Use the SDK's `output_format` to return JSON with `title` and `body` fields.

**C. Two queries**: One for title, one for body.

**Recommendation: A** — convention-based. The LLM writes a `# Title` as the first line, and rloop extracts it as the frontmatter title. This is natural markdown and requires no JSON parsing. The prompt already instructs "A level-1 heading (# Title)" as the first element.

Extraction logic:
```rust
fn extract_title_and_body(enhanced: &str) -> (Option<String>, String) {
    if let Some(first_line) = enhanced.lines().next() {
        if let Some(title) = first_line.strip_prefix("# ") {
            let body = enhanced[first_line.len()..].trim_start();
            return (Some(title.trim().to_string()), body.to_string());
        }
    }
    (None, enhanced.to_string())
}
```

---

## 4. Broader Ancillary Agent Pattern

### The Pattern

Three features need the same thing:

| Feature | Model | Tools | MCP | Structured Output | CWD |
|---------|-------|-------|-----|-------------------|-----|
| Task Enhancement | Sonnet | none | none | none | any (repo root) |
| LLM Verification | Sonnet/Opus | Read, Glob, Grep, Bash | none | optional (pass/fail) | worktree |
| HumanQ Answer Processing | Sonnet | none | none | none | any |

All three are single-query, single-response interactions. None need the `complete` tool, retry logic, or stream interruption. They differ only in prompt, tools, and working directory.

### The `run_query` Abstraction

```rust
// rloop/src/query.rs

pub struct QueryConfig {
    pub model: Model,
    pub cwd: PathBuf,
    pub tools: Vec<BuiltinTool>,
    pub setting_sources: Vec<SettingSource>,  // usually empty
}

pub struct QueryResult {
    pub response: String,
    pub cost_usd: f64,
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub async fn run_query(prompt: &str, config: &QueryConfig) -> Result<QueryResult>;
```

Usage for each feature:

```rust
// Task enhancement
let result = run_query(&enhancement_prompt, &QueryConfig {
    model: Model::Sonnet,
    cwd: repo.to_path_buf(),
    tools: vec![],
    setting_sources: vec![],
}).await?;

// LLM verification
let result = run_query(&verification_prompt, &QueryConfig {
    model: Model::Sonnet,
    cwd: worktree_path.to_path_buf(),
    tools: vec![BuiltinTool::Read, BuiltinTool::Glob, BuiltinTool::Grep, BuiltinTool::Bash],
    setting_sources: vec![SettingSource::Project],
}).await?;

// HumanQ answer processing
let result = run_query(&answer_prompt, &QueryConfig {
    model: Model::Sonnet,
    cwd: repo.to_path_buf(),
    tools: vec![],
    setting_sources: vec![],
}).await?;
```

### When to Add MCP Tools

If a future ancillary query needs custom MCP tools, extend `QueryConfig`:

```rust
pub struct QueryConfig {
    // ...existing fields...
    pub mcp_servers: Option<McpServers>,
    pub output_format: Option<serde_json::Value>,  // for structured output
}
```

But don't add this now. YAGNI. The three known use cases don't need it.

### Relationship to `execute_step`

`run_query` and `execute_step` are fundamentally different:

- `execute_step` is a multi-turn agent session with tool use, verification, retry, and rich event tracking
- `run_query` is a single prompt → single response with optional tool use

They share the same SDK (`ClaudeClient`, `ClaudeAgentOptions`) but the usage pattern is different enough to warrant separate implementations. No need to unify them.

---

## 5. Implementation Sketch — REVISED

### New Files

- `rloop/src/query.rs` — the `run_query` function with `QueryConfig` and `QueryResult`
- Enhancement prompt function in `main.rs` (or a dedicated `enhance.rs` if the prompt is large)

### Changes to Existing Files

- `rloop/src/lib.rs` — add `pub mod query;`
- `rloop/src/main.rs` — revise `TaskAction::New` (always enhances), add `TaskAction::Add` (programmatic), update `cmd_task_new` to call `run_query` with structured output
- `rloop/src/config.rs` — add `[task]` section with `enhance_model`

### CLI Changes

```rust
enum TaskAction {
    /// Human workflow: "here's a rough idea, create tasks for me"
    /// Always runs LLM. Can create multiple tasks. Infers deps.
    New {
        description: String,
        #[arg(long)]
        model: Option<ModelArg>,       // override enhance model
        #[arg(long)]
        verification: Vec<String>,     // applied to all created tasks
    },
    /// Programmatic workflow: "write exactly this task"
    /// No LLM. Takes explicit fields. Body from --body or stdin.
    Add {
        #[arg(long)]
        title: String,
        #[arg(long)]
        body: Option<String>,          // if absent, read from stdin
        #[arg(long)]
        model: Option<ModelArg>,
        #[arg(long = "depends-on", value_delimiter = ',')]
        depends_on: Vec<u32>,
        #[arg(long)]
        verification: Vec<String>,
    },
    Show { id: u32 },
    List,
}
```

### Structured Output Schema

```rust
#[derive(Deserialize, JsonSchema)]
struct EnhancedTasks {
    tasks: Vec<EnhancedTask>,
}

#[derive(Deserialize, JsonSchema)]
struct EnhancedTask {
    title: String,
    description: String,       // full markdown body
    depends_on: Vec<u32>,      // existing task IDs or sibling task IDs
    model: Option<String>,     // suggested model (Haiku/Sonnet/Opus)
}
```

### Flow for `rloop task new "rough idea"`

```
1. Parse CLI args
2. Load all existing tasks (id, title, completed, depends_on)
3. Build enhancement prompt:
   - System: task planning instructions, output schema
   - Context: CLAUDE.md (via project settings), existing task summaries
   - User: the rough description
4. Call run_query(prompt, QueryConfig { model: enhance_model, tools: [Read, Glob, Grep], output_format: EnhancedTasks schema })
5. Parse structured output → Vec<EnhancedTask>
6. For each task:
   a. Assign next sequential ID
   b. Build frontmatter (id, model, depends_on, verification, completed: false)
   c. Write task file
7. git add all new files + commit
8. Print summary: "Created N tasks: 05 (title), 06 (title), 07 (title)"
```

### Flow for `rloop task add --title "..." --body "..."`

```
1. Parse CLI args (read body from stdin if --body absent)
2. Generate next task ID
3. Build frontmatter + body
4. Write task file
5. git add + commit
6. Print: "Created task 05: {title}"
```

---

## 6. Trade-off Summary — REVISED

The two-command approach eliminates the trade-off matrix from the original design:

| Command | LLM | Latency | Cost | Use Case |
|---------|-----|---------|------|----------|
| `rloop task new` | Always | 5-30s (tools + LLM) | ~$0.01-0.05 | Human creates tasks from rough ideas |
| `rloop task add` | Never | Instant | $0 | Agents/scripts create tasks programmatically |

### v1 Scope

1. `rloop::query::run_query()` — shared query function with generic structured output
2. `rloop task new "idea"` — human workflow, always LLM, multi-task, dep inference
3. `rloop task add --title "..." --body "..."` — programmatic workflow, no LLM
4. Config `[task]` section with `enhance_model` (default Sonnet)
5. Enhancement LLM gets Read/Glob/Grep tools and project settings
6. Structured output via schemars for task array parsing
7. Full event logging for all LLM interactions (same detail as execute_step)

---

## 7. Open Questions — RESOLVED

### CLI Design — REVISED

The original research proposed `--enhance` flag + `rloop task refine`. After discussion, the design was fundamentally revised:

**Two commands, clear separation of intent:**

- **`rloop task new "rough idea"`** — Human workflow. Always runs LLM. Loads CLAUDE.md (via project settings) + all existing task summaries. Can create **multiple tasks** with inferred dependencies. Uses structured output (`EnhancedTasks` schema). Gets Read/Glob/Grep tools for codebase exploration. This is a task planning command, not just a text rewrite.

- **`rloop task add --title "..." --body "..."`** — Programmatic/agent workflow. No LLM. Takes explicit fields. Supports stdin for body (if `--body` not provided). Fast, deterministic, zero cost.

No `rloop task refine` for now. No `auto_enhance` config. No `--enhance`/`--no-enhance` flags. The two commands have clear, non-overlapping purposes.

### Resolved Questions

1. **Opt-in or opt-out?** → N/A. `new` always enhances. `add` never does. No flag needed.

2. **Enhancement model?** → **Sonnet default**, configurable via `[task] enhance_model` in config.toml. Overridable per-invocation with `--model`. Even with CLAUDE.md context and codebase tools, Sonnet is sufficient for task structuring.

3. **Should enhancement generate verification commands?** → **No.** Verification commands come from the human via `--verification`. The LLM should not guess build system commands.

4. **Should enhancement infer dependencies?** → **Yes.** Since `new` loads all existing tasks, it can and should infer `depends_on` for the tasks it creates. This was revised from the original "no" recommendation once the command was scoped as a planning tool rather than a text rewrite.

5. **What happens if the LLM query fails?** → **Fail the command.** Print error, exit, create no files. Since `new` always requires LLM, there's no meaningful raw fallback. User retries. (For the raw/programmatic path, use `add`.)

6. **Should `refine` overwrite or create a new version?** → N/A. No `refine` command for now.

7. **Should enhancement read the codebase?** → **Yes.** `new` gets Read, Glob, Grep tools so the LLM can explore the codebase and write more specific tasks (referencing real paths, types, function names).

8. **Should the prompt include CLAUDE.md?** → **Yes, via project settings.** `setting_sources` matches whatever the step executor uses (project-level). CLAUDE.md loads automatically through the SDK.

9. **How does this interact with the create-rloop-tasks skill?** → They serve different workflows. `rloop task new` works from the rloop CLI. The skill works from a Claude Code session. They don't conflict.

10. **Should `rloop task refine` work on completed tasks?** → N/A. No `refine` for now.

### Additional Decisions

11. **Streaming:** Non-streaming. `run_query` runs silently and returns the result. But all events (messages, tool calls, token usage) are logged with the same detail as `execute_step` for full history.

12. **Structured output:** `run_query` supports generic structured output via `QueryConfig.output_format: Option<serde_json::Value>`. Callers pass `schemars::schema_for!(TheirType)`. Task enhancement uses `EnhancedTasks { tasks: Vec<EnhancedTask> }`. LLM verification (research 03) will use `LlmVerificationResult`. Generic by design.

13. **Stdin for `add`:** `rloop task add` reads body from stdin if `--body` is not provided. Title and deps are always flags.
