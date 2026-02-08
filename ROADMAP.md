# rloop Roadmap

**Last Updated:** 2026-02-08

rloop v1 is complete — 267 tests, parallel DAG execution, merge-fix agent, full CLI. This roadmap evolves rloop from a local CLI tool into a distributed system with a database-backed brain, pull-model workers, and live monitoring.

## Architecture

```
                    ┌──────────────┐
                    │   Supabase   │
                    │  (Postgres)  │
                    └──────┬───────┘
                           │
                    ┌──────┴───────┐
         ┌─────────┤    Brain     ├─────────┐
         │         │  (axum API)  │         │
         │         └──────┬───────┘         │
         │                │                 │
    ┌────┴────┐    ┌──────┴───────┐   ┌─────┴─────┐
    │   CLI   │    │    Worker    │   │  Web/TUI   │
    │  (thin) │    │  (local PC)  │   │  (client)  │
    └─────────┘    └──────────────┘   └───────────┘
                          │
                   ┌──────┴───────┐
                   │    Worker    │
                   │  (cloud VM)  │
                   └──────────────┘
```

**Three roles:**

- **Brain** — Coordinator and single gateway. Connects to Postgres, resolves DAGs, schedules tasks to workers, enforces state transitions, broadcasts events to clients. Stateless — can run anywhere with a DB connection.
- **Worker** — Execution engine. Has filesystem access to git repos. Creates worktrees, runs Claude agents, reports results back to brain. Pulls tasks (brain doesn't push). Can run multiple in parallel.
- **Database** — Supabase (Postgres). Source of truth for everything. `supabase start` runs locally in Docker, `supabase db push` deploys the same schema to hosted.

**Communication pattern:**

```
COMMANDS (writes)              QUERIES (reads)
─────────────────              ───────────────
CLI    ──→ Brain API           TUI ──→ Brain API
Web    ──→ Brain API           Web ──→ Brain API (WebSocket)
Worker ──→ Brain API           CLI ──→ Brain API

Worker ──events──→ Brain ──writes──→ DB
                     │
                     ├──stream──→ TUI (WebSocket)
                     ├──stream──→ Web app (WebSocket)
                     └──stream──→ CLI (optional)
```

The brain is the single gateway. Workers push events to it, clients pull from it. Postgres is an implementation detail behind the brain.

## Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Database | Supabase (Postgres) | Real-time subscriptions, local dev via Docker, sqlx compile-time checks, same schema local/hosted |
| Transport | axum HTTP from the start | Workers, web, TUI all use HTTP. No Unix socket to replace later. Bind 127.0.0.1 for local, change bind + add auth for remote. |
| Task storage | Postgres, not files | No more YAML frontmatter parsing, no commits to flip `completed: true`, agents get task content via MCP tools instead of file reads |
| Client→DB | All through brain API | Brain is the single gateway. Direct DB access from clients is the wrong pattern — brain already has the event stream, just broadcast it. |
| Worker model | Pull, not push | Worker calls `GET /work`, brain doesn't need to know worker addresses |
| `rloop run` | Defaults to all tasks | `rloop run` = all incomplete, `rloop run 05` = specific target. `--all` flag is unnecessary ceremony. |
| Language | Rust everywhere | Already invested. ratatui for TUI, axum for brain, sqlx for DB. |

## v1 Module Migration

Where current modules land in the distributed architecture:

| v1 Module | Destination | Notes |
|---|---|---|
| `task.rs` (parsing, DAG) | Brain | Resolves execution order, but tasks come from Postgres not files |
| `session.rs` (orchestration) | Brain | Schedules tasks to workers, tracks session state |
| `step.rs` (agent execution) | Worker | Runs the actual Claude agent. `StepExecutor` trait becomes the worker boundary. |
| `git.rs` (worktrees) | Worker | Local filesystem operations |
| `tools.rs` (MCP tools) | Worker | Served to the agent in the worktree |
| `merge.rs` (merge-fix agent) | Worker | Runs in task worktree, reports result to brain |
| `log.rs` (events) | Brain | Receives events from workers, writes to DB, broadcasts to clients. `EventSender` channel maps to worker→brain event stream. |
| `config.rs` | Split | Project config in DB (brain), worker runtime config local |

---

## Milestone 1: Database + Brain Foundation

**Goal:** Replace file-based tasks with Postgres. Run brain as a daemon. CLI becomes a thin client.

### What gets built

**Supabase local setup**
- `supabase init` + `supabase start` in dev workflow
- Docker Compose for local Postgres + Supabase services

**Postgres schema**

```sql
-- Projects
CREATE TABLE projects (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL,
    path        TEXT NOT NULL,      -- local filesystem path
    created_at  TIMESTAMPTZ DEFAULT now()
);

-- Tasks (replaces .rloop/tasks/*.md files)
CREATE TABLE tasks (
    id          SERIAL PRIMARY KEY,
    project_id  UUID REFERENCES projects(id),
    title       TEXT NOT NULL,
    description TEXT NOT NULL,       -- markdown body
    model       TEXT NOT NULL DEFAULT 'Sonnet',
    depends_on  INTEGER[] DEFAULT '{}',
    verification TEXT,               -- command string
    completed   BOOLEAN DEFAULT false,
    created_at  TIMESTAMPTZ DEFAULT now()
);

-- Sessions
CREATE TABLE sessions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID REFERENCES projects(id),
    target_task INTEGER,             -- NULL = all tasks
    branch      TEXT NOT NULL,
    parent_branch TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'running',  -- running, completed, failed, cancelled
    auto_merge  BOOLEAN DEFAULT false,
    started_at  TIMESTAMPTZ DEFAULT now(),
    finished_at TIMESTAMPTZ
);

-- Events (replaces JSONL session logs)
CREATE TABLE events (
    id          BIGSERIAL PRIMARY KEY,
    session_id  UUID REFERENCES sessions(id),
    task_id     INTEGER,
    event_type  TEXT NOT NULL,
    payload     JSONB NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT now()
);
```

**Brain daemon (axum)**
- `rloopd` binary — starts brain, connects to local Supabase
- API endpoints:
  - `POST /projects` — register a project
  - `POST /sessions` — start a run (brain resolves DAG, queues tasks)
  - `POST /sessions/:id/cancel` — cancel a session
  - `GET /projects/:id/tasks` — list tasks
  - `POST /projects/:id/tasks` — create task
  - `GET /sessions/:id` — session status
  - `GET /sessions/:id/events` — session event log

**CLI as thin client**
- `rloop init` → registers project with brain
- `rloop run` → `POST /sessions` to brain, streams back events
- `rloop task list` → `GET /tasks` from brain
- `rloop task add` → `POST /tasks` to brain

**Worker embedded in brain (monolith)**
- Brain + worker in one process for now
- Current `step.rs` / `git.rs` / `merge.rs` run locally
- Worker reports events to brain via in-process channel (same `EventSender` pattern)

### What this changes

- Tasks live in Postgres, not `.rloop/tasks/*.md`
- Session logs go to the `events` table, not JSONL files
- `rloop` CLI talks to brain API, doesn't execute directly
- `.rloop/` directory shrinks to just `config.toml` (project ID + local overrides) and `worktrees/` (gitignored)

### Success criteria

- `rloop init` registers project in DB
- `rloop task add` creates task in Postgres
- `rloop run` executes session through brain daemon
- `rloop task list` shows tasks from DB
- Session events stored in Postgres, queryable

---

## Milestone 2: Event Streaming + Monitoring TUI

**Goal:** Live monitoring of sessions via a ratatui TUI. Brain broadcasts events over WebSocket.

### What gets built

**WebSocket event stream on brain**
- `GET /events` (WebSocket) — subscribe to live events
- Filters: by project, session, task
- Brain broadcasts every event it receives from the worker

**TUI application (ratatui)**
- Separate binary or mode: `rloop tui`
- Connects to brain API (HTTP for state, WebSocket for live events)
- Views:
  - **Dashboard** — active sessions, recent completions, running workers
  - **Session view** — live event stream for a session (task starts, tool calls, verification, completions)
  - **Task list** — all tasks with status, dependencies, completion
  - **Task detail** — full task spec, attempt history, cost/token metrics
  - **Session history** — browse past sessions, drill into event logs

**Terminal output preserved**
- Current colored terminal output still works for `rloop run` (streams from brain)
- TUI is for monitoring, not for running commands

### Success criteria

- Brain streams events over WebSocket
- TUI shows live session progress
- TUI can browse past sessions and task history
- Works alongside `rloop run` terminal output

---

## Milestone 3: Worker Separation

**Goal:** Worker becomes a separate process. Brain schedules, worker pulls and executes.

### What gets built

**Worker pull protocol**
- Worker binary: `rloop-worker`
- On startup: connects to brain, registers itself
- Poll loop: `GET /work` — brain returns next task assignment or 204
- Assignment includes: project path, task content, config, session context
- Worker executes step (current `step.rs` logic), streams events back to brain
- On completion: reports `StepResult` to brain

**Brain scheduler**
- Resolves DAG for session, determines which tasks are ready
- Serves ready tasks to workers via `/work` endpoint
- Tracks which worker is running which task
- Handles worker disconnects (mark task as failed, available for retry)

**Brain API additions**
- `GET /work` — worker pulls next assignment
- `POST /work/:id/events` — worker streams events
- `POST /work/:id/complete` — worker reports step result
- `GET /workers` — list connected workers (for TUI)

**Worker configuration**
- `rloop-worker --brain http://localhost:3000`
- Optional: `--parallel 3` (run N tasks concurrently)
- Worker needs: git, Claude API key, filesystem access to project repos

### What this changes

- Brain no longer runs agents — it just coordinates
- Workers can run on different machines (if they have repo access)
- Multiple workers can run in parallel against the same brain
- `StepExecutor` trait becomes the process boundary

### Success criteria

- Worker pulls tasks from brain and executes them
- Brain schedules DAG correctly across workers
- Multiple workers can run in parallel
- Worker crash doesn't crash the brain
- TUI shows worker status

---

## Milestone 4: Human Queue

**Goal:** Agents can prompt humans. Questions queue in the brain, humans review in batch.

### What gets built

**MCP tools for agents**
- `ask_question(text, options?, blocking?)` — agent queues a question for human review
- `log_assumption(decision, reasoning)` — agent logs a decision it made
- `propose_task(title, description)` — agent proposes new work

**Queue schema**

```sql
CREATE TABLE queue_items (
    id          BIGSERIAL PRIMARY KEY,
    project_id  UUID REFERENCES projects(id),
    session_id  UUID REFERENCES sessions(id),
    task_id     INTEGER,
    item_type   TEXT NOT NULL,        -- question, assumption, proposal
    status      TEXT NOT NULL DEFAULT 'pending',  -- pending, answered, acknowledged, approved, rejected
    blocking    BOOLEAN DEFAULT false,
    text        TEXT NOT NULL,
    options     TEXT[],               -- for questions with choices
    reasoning   TEXT,                 -- for assumptions
    response    TEXT,                 -- human's answer
    created_at  TIMESTAMPTZ DEFAULT now(),
    resolved_at TIMESTAMPTZ
);
```

**Brain queue API**
- `POST /queue` — worker submits queue item (from agent MCP tool)
- `GET /queue` — list pending items (for TUI/web)
- `POST /queue/:id/respond` — human answers/acknowledges/approves
- Brain notifies worker when a blocking question is answered

**TUI queue view**
- List pending items by type (questions, assumptions, proposals)
- Keyboard-driven: answer questions, acknowledge assumptions, approve/reject proposals
- Batch review — zero wait time between items

**Worker blocking behavior**
- Non-blocking items: agent continues, human reviews later
- Blocking items: agent pauses, brain holds task, resumes when answered
- Brain routes answers back to the correct worker/agent session

### What this changes

- Agents can ask for help instead of guessing
- Humans review in batches, not synchronously
- Proposed tasks go to the queue, human approves, brain creates them
- Context injection: answered questions for a task are included in retry prompts

### Success criteria

- Agent can queue questions via MCP tool
- Human can answer in TUI
- Blocking questions pause the agent, answers resume it
- Assumptions are logged and reviewable
- Task proposals go through approval flow

---

## Milestone 5: Web App

**Goal:** Remote monitoring and queue review via a web interface. Accessible over Tailscale.

### What gets built

**Next.js web application**
- Connects to brain API (same endpoints as TUI)
- WebSocket for live event streaming
- Views mirror TUI: dashboard, sessions, tasks, queue, logs
- Queue review: answer questions, review assumptions, approve proposals
- Responsive — works on phone for quick queue review

**Brain API CORS + auth**
- CORS headers for web app origin
- API key auth (simple bearer token for personal use)
- Rate limiting not needed for personal use

**Tailscale access**
- Brain binds to Tailscale IP (or 0.0.0.0 with Tailscale ACLs)
- Web app served locally or deployed to Vercel
- Brain API accessible from any Tailscale device

### Success criteria

- Web app shows live session progress
- Queue items can be reviewed from phone
- Accessible over Tailscale from any device
- Same data as TUI, different interface

---

## Milestone 6: Cloud Deployment

**Goal:** Run brain and workers in the cloud. Hosted Supabase. Remote execution.

### What gets built

**Hosted Supabase**
- `supabase db push` deploys schema to hosted instance
- Brain connects to hosted Postgres URL
- Same schema, same migrations, different connection string

**Brain in cloud**
- Deploy brain to a VPS or container service
- Public HTTPS endpoint (behind Tailscale or with proper auth)
- CLI and TUI connect to remote brain

**Cloud workers**
- Workers run on cloud VMs with git repos checked out
- Workers pull from remote brain, execute steps, push events back
- Cloud workers + local workers can coexist

**Auth and security**
- API key auth for brain endpoints
- Worker authentication (shared secret or mTLS)
- Project-level access control (future: multi-user)

### Success criteria

- Brain runs in cloud, accessible from anywhere
- Workers run on remote VMs
- Local CLI/TUI connects to cloud brain
- Web app connects to cloud brain
- Same workflow as local, just remote

---

## Context Injection Model

Across all milestones, the agent's context evolves:

```
┌─────────────────────────────────────────────────────────────────┐
│                    TASK CONTEXT (what agent sees)                │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ STATIC CONTEXT (from project)                            │   │
│  │ - CLAUDE.md (project instructions, architecture)         │   │
│  │ - Skills and agents (project-level only)                 │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ DYNAMIC CONTEXT (from brain, via MCP tools)              │   │
│  │ - Current task details (from DB, not filesystem)         │   │
│  │ - Previous attempt learnings (accumulated in brain)      │   │
│  │ - Answered questions relevant to this task               │   │
│  │ - Related task summaries (dependencies, siblings)        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ INSTRUCTIONS (from rloop)                                │   │
│  │ - How to call `complete` (signal task done)              │   │
│  │ - How to call `ask_question` (M4+)                       │   │
│  │ - How to call `log_assumption` (M4+)                     │   │
│  │ - How to call `propose_task` (M4+)                       │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Key shift from v1:** Agents access task content via MCP tools (`get_task`, `list_tasks`), not by reading `.rloop/tasks/03.md`. The tools can return richer context — dependency status, previous attempt summaries, related queue items — that a flat file never could.

## Open Questions

1. **Worker repo sync** — How do cloud workers get the repo? Git clone + pull? Mounted volume? This determines cloud worker setup complexity.
2. **Multi-project sessions** — Can the brain run sessions for multiple projects concurrently? Probably yes (different workers), but needs thought on resource limits.
3. **Session resume** — When a session is cancelled, can it be resumed? Brain has all state in DB, so this is feasible but needs protocol design.
4. **Task versioning** — When a task spec changes mid-session, what happens? Probably: session uses the spec it started with, new runs get the new spec.
