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

## 5. Implementation Sketch

### New Files

- `rloop/src/query.rs` — the `run_query` function (~100 lines)
- Enhancement prompt constant or function in `main.rs` (or a dedicated `enhance.rs` if the prompt is large)

### Changes to Existing Files

- `rloop/src/lib.rs` — add `pub mod query;`
- `rloop/src/main.rs` — add `--enhance` flag to `TaskAction::New`, add `TaskAction::Refine`, update `cmd_task_new` to optionally call enhancement
- `rloop/src/config.rs` — add `[task]` section with `auto_enhance` and `enhance_model`

### CLI Changes

```rust
enum TaskAction {
    New {
        description: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        model: Option<ModelArg>,
        #[arg(long = "depends-on", value_delimiter = ',')]
        depends_on: Vec<u32>,
        #[arg(long)]
        verification: Vec<String>,
        #[arg(long)]
        enhance: bool,           // NEW
        #[arg(long)]
        no_enhance: bool,        // NEW (overrides config default)
    },
    Show { id: u32 },
    List,
    Refine {                     // NEW
        id: u32,
        #[arg(long)]
        model: Option<ModelArg>,
    },
}
```

### Flow for `rloop task new --enhance`

```
1. Parse CLI args
2. Generate next task ID
3. Build raw task content (frontmatter + description)
4. If enhance:
   a. Build enhancement prompt with raw description
   b. Call run_query(prompt, Sonnet)
   c. Extract title from first # heading (if no --title given)
   d. Replace description body with enhanced version
5. Write task file
6. git add + commit
7. Print summary (including cost if enhanced)
```

### Flow for `rloop task refine <id>`

```
1. Load existing task file
2. Build enhancement prompt with current description
3. Call run_query(prompt, Sonnet)
4. Extract title (if current title is empty)
5. Write updated task file (preserve frontmatter, replace body)
6. git add + commit
7. Print summary
```

---

## 6. Trade-off Summary

| Approach | Latency | Cost | User Control | Complexity |
|----------|---------|------|--------------|------------|
| `--enhance` flag (opt-in) | +5-15s when used | ~$0.001-0.005/task | High | Low |
| Config default (opt-out) | +5-15s always | Adds up over many tasks | Medium | Low |
| Separate `refine` command | No change to `new` | On demand | High | Medium |
| Interactive draft-confirm | +5-15s + user time | Same | Highest | High |
| No enhancement | None | None | N/A | None |

### Recommended v1 Scope

1. `rloop::query::run_query()` — shared lightweight query function
2. `--enhance` flag on `rloop task new` — opt-in enhancement
3. `rloop task refine <id>` — enhance existing tasks
4. Config `[task]` section with `auto_enhance` and `enhance_model`
5. Sonnet as default enhancement model (fast, cheap, good at writing)
6. Hardcoded system prompt with 2-3 curated examples
7. Title auto-generation from the `# Heading` convention

---

## 7. Open Questions

### Must Resolve Before Implementation

1. **Opt-in or opt-out?** Should `auto_enhance` default to `false` (opt-in) or `true` (opt-out)? Recommendation: `false` — users should explicitly choose to spend tokens. They can flip it in config once they see the value.

2. **Enhancement model?** Sonnet is the sweet spot (fast, cheap, good writing). Haiku is too terse for task writing. Opus is overkill and slow for a text rewrite. Allow override via config and `--model` flag.

3. **Should enhancement generate verification commands?** Probably not — verification commands require understanding the project's build system and test framework. The user is better positioned to specify these. The LLM could suggest them in a comment, but auto-generating them risks broken commands.

4. **Should enhancement infer dependencies?** No — dependency inference requires understanding the full task DAG and what each task does. This is a fundamentally different problem (task planning) and should not be conflated with text enhancement.

5. **What happens if the LLM query fails?** Fall back to the raw description. Print a warning ("enhancement failed, using original description"). Never block task creation on an LLM failure.

### Worth Discussing

6. **Should `refine` overwrite or create a new version?** Overwrite in place (same file, same ID). The user can always `git diff` to see what changed. Version tracking is what git is for.

7. **Should enhancement read the codebase?** No tools for v1. The enhancement is purely a text rewrite — improve structure, add acceptance criteria, clarify language. Codebase-aware enhancement (reading files to add specific paths and types) would require tools and a larger context, making it slower and more expensive. Consider this as a future "deep enhance" mode.

8. **Should the prompt include the project's CLAUDE.md?** Maybe — it contains coding conventions that could inform acceptance criteria. But it adds tokens and the enhancement prompt should be self-contained. Worth testing.

9. **How does this interact with the create-rloop-tasks skill?** The PRD mentions a `.claude/skills/create-rloop-tasks/SKILL.md` skill for task creation from Claude Code. Enhancement via `--enhance` is the CLI equivalent — both produce well-structured tasks, but `--enhance` works from the rloop CLI without needing a Claude Code session. They serve different workflows and don't conflict.

10. **Should `rloop task refine` work on completed tasks?** Yes — you might want to refine a task's description for documentation purposes even after it's done. The `completed` field stays unchanged.
