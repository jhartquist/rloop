# rloop — Product Requirements

## What is rloop?

rloop executes tasks in isolated git worktrees using Claude. You write task files (precise specs), point rloop at one, and it resolves dependencies, creates worktrees, runs the agent, verifies the result, and merges the work. The goal: write tasks so precisely that execution is near-deterministic.

## Core Concepts

### Task

A markdown file with YAML frontmatter. Lives in `.rloop/tasks/`. Immutable during execution — all runtime state is in memory. The session branch may update the `completed` field and commit it as part of the result.

```markdown
---
id: "03"
model: opus
depends_on: ["01", "02"]
verification: "cargo test"      # optional, overrides project default
completed: false
---

# Implement audio preprocessing

Read audio files, split into frames, apply FFT to get frequency bins.

## Acceptance Criteria

- Reads .wav files via `hound` crate
- Frames at 1024 samples with 512 hop
- FFT via `rustfft`, output is magnitude spectrum
- Unit tests cover mono 16-bit and stereo conversion
```

**Numbering:** `00.md`, `01.md`, `02.md`, etc. If we exceed 100 tasks, rename all to three digits.

**Dependencies:** The `depends_on` field references other task IDs. rloop resolves these into a DAG and runs them in order (sequential for now, parallel later).

**Completed:** Set to `true` on the session branch after successful verification. rloop skips completed tasks when resolving dependencies. Clear it to re-run.

### Step

A step is one execution attempt of a task. Within a step, the agent works (reads, edits, runs commands), then calls the `complete` tool to signal it's done. The system runs verification. If verification fails, the agent gets the output and retries. A task may take multiple steps (retries) within a single session.

```
step(task, worktree) -> Success { summary } | Failed { reason, learnings }
```

### Session

A session is one invocation of rloop. You target a single task. rloop resolves that task's dependency DAG, runs each dependency that isn't already completed, merges each into the target branch, then runs the target task itself.

A session produces:
- Code changes on a branch
- Updated task files (`completed: true`) on that branch
- Potentially new task files created by the agent (future)
- Session logs in `.rloop/sessions/`

### Worktree

Each task executes in its own git worktree, branched off the session branch. The agent can only access files within the worktree. On success, the task's worktree is merged into the session branch and cleaned up. On failure, the worktree is cleaned up and the failure is logged.

## Execution Model

### Running a Task

```
rloop run 05
```

1. **Initialize** — ensure `.rloop/` exists, load config
2. **Load task** — read `.rloop/tasks/05.md`, parse frontmatter
3. **Resolve DAG** — find all transitive dependencies, topological sort
4. **Create session branch** — branch off current HEAD (main, feature branch, whatever)
5. **For each dependency (in order):**
   a. Skip if already `completed: true`
   b. Create worktree off session branch
   c. Execute step (agent works, verification, retry up to N times)
   d. On success: merge worktree into session branch, mark completed, cleanup worktree
   e. On failure: log failure, cleanup worktree, abort session
6. **Run target task** — same as 5b-5d
7. **Result:**
   - Session branch has all changes + updated task files
   - `--auto-merge`: merge session branch back to parent
   - Default: leave session branch for human review

### Running All Tasks

```
rloop run --all
```

Same as above but the "target" is a virtual root that depends on every incomplete task. Creates a session branch, runs everything in DAG order, result is a single branch to review or auto-merge.

### Verification & Retry

When the agent calls the `complete` tool:
1. System runs the verification command(s) in the worktree
2. **Pass:** task succeeds, step is done
3. **Fail:** verification output is returned to the agent as an error, agent retries
4. Max retry attempts: configurable, default ~10

If the agent exits without calling `complete` (runs out of turns, decides it can't proceed), structured output captures the failure reason and learnings. These learnings are accumulated in memory and included in the prompt if the task is retried within the same session.

Verification is optional. If no verification command is configured (per-task or per-project), calling `complete` is sufficient for success.

### Failure & Learnings

Within a session, failure learnings are accumulated in memory:
- Step 1 fails with "forgot to add the dependency to Cargo.toml"
- Step 2 (retry) gets the original task + "Previous attempt failed: forgot to add the dependency to Cargo.toml"
- This continues until success or max retries

Between sessions, the session log captures everything. The human can review logs and improve the task file for the next run. Task files themselves are never modified by the system except for the `completed` field on the session branch.

## Git & Worktree Strategy

### Branch Naming

```
Session branch: rloop/{target-task-id}        e.g. rloop/05
Task worktree:  rloop/{task-id}               e.g. rloop/03
```

Session branches are based off current HEAD. Task worktrees branch off the session branch.

### Worktree Lifecycle

```
1. CREATE    git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}
2. LOCK      git worktree lock (prevent accidental removal)
3. WORK      Agent operates in worktree directory
4. VERIFY    Run verification commands in worktree
5. MERGE     Merge worktree branch into session branch
6. CLEANUP   Unlock, remove worktree, delete branch
```

### Auto-Merge

`--auto-merge` controls whether the session branch merges back to the parent after all tasks complete:

```bash
rloop run 05                 # session branch left for review
rloop run 05 --auto-merge    # session branch merged to parent, cleaned up
```

For single tasks without dependencies, auto-merge effectively goes straight to the parent branch.

### Assumptions

- One session at a time (no concurrent rloop runs on the same repo)
- All reviewable changes are merged before starting a new session
- Git must be initialized in the project
- Future daemon will handle concurrent session tracking

## Custom MCP Tools

Two custom tools available to the agent:

### `complete`

Signals the agent is done with the task. Triggers verification.

```json
{
  "name": "complete",
  "description": "Call when you have finished the task.",
  "parameters": {
    "summary": { "type": "string", "description": "Brief summary of what was done" }
  }
}
```

On call:
- Run verification command(s) in the worktree
- If pass: return success, interrupt agent stream
- If fail: return error with verification output, agent continues

### `context_usage`

Returns the agent's current context window utilization.

```json
{
  "name": "context_usage",
  "description": "Check how much of your context window you have used.",
  "parameters": {}
}
```

Returns: `{ "percentage": 47.3, "recommendation": "plenty of room" }`

The agent can use this to make decisions: try a different approach if running low, wrap up early, or report that the task is too large. No hard cutoff from the system — the agent manages itself. Threshold for concern: ~60-70%.

## Agent Environment

### What the agent gets

- **Working directory:** the worktree (sandboxed)
- **System prompt:** task framing, instructions to call `complete` when done
- **User message:** task description + any failure learnings from retries
- **Project settings:** `.claude/skills/` and `.claude/agents/` loaded (project-level only)
- **CLAUDE.md:** loaded automatically via project settings
- **Tools:** all built-in tools enabled (Read, Write, Edit, Bash, Glob, Grep, etc.)
- **Custom MCP tools:** `complete`, `context_usage`
- **Sibling context (future):** summaries of completed/upcoming tasks

### What the agent does NOT get

- Global/user-level settings or skills
- Ambient context from outside the worktree
- Access to files outside the worktree directory

## Session Logging

### Full Log (`.rloop/sessions/{session-id}.jsonl`)

Every event written as JSONL. One line per event. Includes both Claude stream events and rloop lifecycle events:

**Claude events:**
- Assistant messages (text content)
- Tool calls (name, input)
- Tool results (name, output)
- Token usage per message

**rloop lifecycle events:**
- Task started / task completed / task failed
- Worktree created / worktree cleaned up
- Prompt sent to agent
- Verification attempted (command, pass/fail, output)
- Worktree merged into session branch
- Session branch ready for review
- Session branch auto-merged to parent
- Session cancelled (Ctrl+C)
- Timestamps on everything

This is the raw record for programmatic analysis.

### Terminal Output

Pretty-printed, colored, human-friendly. Shows:
- Assistant text (what the agent is saying/thinking)
- Tool calls: which file read/written/edited, with contents or diffs
- Verification results: pass/fail with output
- Context usage: percentage, shown periodically
- Step transitions: "Starting task 03...", "Task 03 complete", etc.

Hides:
- Raw JSON event envelopes
- Per-message token counts
- SDK internals

### Context Tracking

Tracked incrementally from stream messages. Displayed as a simple percentage in terminal output. Logged per-step in session logs for analysis (which tasks consume the most context).

## Configuration

### `.rloop/config.toml`

```toml
[step]
model = "opus"
max_turns = 50
max_retries = 10

[logging]
session_dir = ".rloop/sessions"
```

Per-task frontmatter specifies verification commands (if any) and can override model.

### `.rloop/` Directory Structure

```
.rloop/
├── config.toml          # project configuration (committed)
├── tasks/               # task files (committed)
│   ├── 00.md
│   ├── 01.md
│   └── 02.md
├── worktrees/           # active worktrees (gitignored)
│   ├── 00/
│   └── 01/
└── sessions/            # session logs (gitignored)
    └── {session-id}.jsonl
```

**Committed:** `config.toml`, `tasks/`
**Gitignored:** `worktrees/`, `sessions/`

rloop auto-initializes `.rloop/` if it doesn't exist (creates directories, default config, adds gitignore entries).

### Task Creation

Tasks are created manually or with the help of a Claude Code skill:

**`.claude/skills/create-rloop-tasks/SKILL.md`** — a project-level skill that knows the task file format and can generate tasks from a description of work. Invoked from any Claude session with `/create-rloop-tasks`.

`rloop init` scaffolds an example task in `.rloop/tasks/` and installs the skill, so the format is documented from the start.

## Cancellation

**Ctrl+C** during execution:
1. Interrupt the Claude stream
2. Clean up all active worktrees (unlock, remove, delete branches)
3. Log the cancellation to the session log
4. Exit cleanly

For multi-task sessions, cancellation kills everything. No partial resume — start fresh.

## CLI (Initial)

Start simple, add structure later:

```bash
# Initialize .rloop/ in current repo
rloop init

# Run a task (by ID, resolves DAG)
rloop run 05

# Run all incomplete tasks
rloop run --all

# Run with auto-merge
rloop run 05 --auto-merge

# List tasks and their status
rloop list
```

The CLI will start as a simple `main.rs` and evolve. Clap can be added when the interface stabilizes.

## What We Are NOT Building

- GitHub/PR integration (no `gh` commands)
- External task management (no `br` integration)
- New project/repo creation
- Background task execution
- TUI (terminal UI)
- sccache integration (environment concern)

## Deferred Features

These are designed for but not built in v1:

- **Parallel DAG execution** — run independent tasks concurrently (design sequential, architecture supports parallel)
- **Task creation by agents** — agents can propose new task files during execution
- **Human queue** — ask_question, log_assumption, propose_task tools
- **LLM verification** — use a model to verify output quality beyond command-line checks
- **Sibling task context** — inject summaries of completed/upcoming tasks into agent prompt
- **Tool presets** — named tool configurations (research, code, review)
- **Session resume** — continue a cancelled or failed session
- **Daemon mode** — long-running process tracking multiple concurrent sessions

## Success Criteria

1. A single task runs in an isolated worktree and produces verified output
2. DAG dependencies resolve correctly and run in order
3. Failed steps retry with learnings and succeed on subsequent attempts
4. Session branch captures all changes + task status for human review
5. Session logs capture enough detail for future optimization analysis
6. The same task files can be run multiple times to compare results
