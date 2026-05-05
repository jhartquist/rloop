# rloop

DAG-based task executor for AI coding agents. Define tasks as files, run them
in isolated git worktrees, verify with tests, merge on success.

> Built as an experiment in agentic engineering — I designed the architecture,
> drove the implementation through agent loops, and use it daily. Held to a
> strict test + clippy-pedantic bar. The structure may show its origins; the
> behavior is as specified in [VISION.md](VISION.md), [PRD.md](PRD.md), and
> [SPEC.md](SPEC.md).

## What it does

For each task in a DAG:

1. Spawn an isolated git worktree
2. Run a Claude agent against the task description
3. Run verification (tests, clippy, build — whatever you configure)
4. On success: commit and merge back; on failure: capture learnings and retry
5. Move to the next ready task

## Install

```bash
git clone https://github.com/jhartquist/rloop
cd rloop
cargo install --path .
```

Requires a working `cargo` toolchain and the `claude` CLI on PATH.

## Quickstart

```bash
# In your project:
rloop init                              # create .rloop/ directory and config

# Define a task (LLM-enhanced from a rough description):
rloop task new "Add JSON output to the CLI"

# Or add a task with explicit fields:
rloop task add --title "Add JSON output" --verify "cargo test"

# Run a specific task:
rloop run 31

# Run all ready tasks in DAG order:
rloop run --all
```

Tasks live in `.rloop/tasks/<id>.md` as markdown with frontmatter. Edit them
directly — they're just files.

## Other commands

```bash
rloop task list                # list tasks and status
rloop task show <id>           # show full task details
rloop session show <id>        # replay a session log
rloop run --all --auto-merge   # run everything, auto-merge to parent branch
```

## Layout

```
.rloop/
├── config.toml      # parent_branch, verify command, model, max_parallel
├── tasks/           # task definitions (markdown + frontmatter)
├── sessions/        # per-run JSONL logs
└── worktrees/       # isolated worktrees per task (transient)
```

## CLAUDE.md snippet for projects using rloop

Drop this into the `CLAUDE.md` of a project that uses rloop, so Claude knows
the workflow when you're working in that repo:

```markdown
## rloop

This project uses rloop for task-driven development.

- Tasks live in `.rloop/tasks/<id>.md`. Each is markdown body + frontmatter
  (`title`, `verify`, `deps`, optional `model`).
- `rloop run` executes the next ready task in an isolated worktree, runs
  the verification command, and merges on success.
- `rloop task new "<description>"` creates a new task via LLM enhancement;
  `rloop task add --title ...` skips the LLM and is deterministic.
- Do not edit `.rloop/sessions/` — those are run logs.
- Do not commit `.rloop/worktrees/` — they are transient.
```

## Docs

- [VISION.md](VISION.md) — MDP framing, design philosophy
- [PRD.md](PRD.md) — Product requirements
- [SPEC.md](SPEC.md) — Technical design, module layout
- [ROADMAP.md](ROADMAP.md) — Future direction
- [research/](research/) — Long-form notes on parallel DAGs, daemon, LLM
  verification, human queues, context/cost tracking

## License

[MIT](LICENSE-MIT).
