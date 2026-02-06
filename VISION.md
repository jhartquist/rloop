# rloop Vision

## Goal

Build the most powerful step function, then run it in loops to get work done.

Secondary goal: educate. Break everything into small chunks so anyone can learn how it's built and modify it for their own needs.

## The Step is Everything

## Core Insight

If we perfect the step function, the loop is trivial:

```
for task in dag.topological_order() {
    step(task)
}
```

All complexity lives in `step()`. Everything else is orchestration.

## The Step

A step is a single atomic unit of work:

```
step(state, task) → (state', result)
```

**Inputs:**
- `state` - codebase, task files, context
- `task` - what to accomplish

**Outputs:**
- `state'` - modified codebase, updated task status
- `result` - Success | Failed

**What happens inside:**
1. Set up isolated environment (worktree)
2. Agent works (Claude with controlled tools)
3. Verify (tests, build)
4. Transition (commit, merge into session branch)
5. Return result

## MDP Formulation

rloop is a Markov Decision Process:

```
MDP = (S, A, T, R, π)

S = State space (codebase × task_state × context)
A = Action space (a step execution)
T = Transition function: S × A → S'
R = Reward function: S × A → {Success, Failed}
π = Policy: task → step_config
```

### State (S)

Everything observable before a step:
- Codebase (files, git history)
- Task state (completed or not, from task files)
- Previous attempts (failure learnings accumulated in memory, session logs on disk)

### Action (A)

A step execution. Within the step, the agent takes many micro-actions (edits, tool calls), but from the MDP's view, the whole step is one action.

### Transition (T)

Deterministic given the step outcome:
- Success → task marked completed, code merged into session branch, state updated
- Failed → task remains incomplete, learnings captured, retry possible

### Reward (R)

Sparse signal at end of step:
- `Success` - verification passes (or `complete` called with no verification configured), task done
- `Failed` - verification failed after max retries, or agent gave up

### Policy (π)

How we configure each step:

```rust
struct StepConfig {
    model: Model,              // haiku, sonnet, opus
    system_prompt: String,     // task framing
    tools: Vec<Tool>,          // allowed tools
    max_turns: u32,            // budget
    max_retries: u32,          // verification retry limit
    verification: Option<Vec<String>>, // commands to run (per-task)
}
```

The policy maps tasks to configs. Initially from task frontmatter + project config, later learned.

## The Learning Loop

Every step produces a trajectory:

```
τ = [(s₀, a₀), (s₁, a₁), ..., (sₙ, aₙ), reward]
```

We log everything:
- Full session events (Claude messages, tool calls, results)
- rloop lifecycle events (worktree create, merge, cleanup)
- Config used (model, prompt, tools)
- Metrics (tokens, duration, tool call counts, context percentage)
- Outcome (Success/Failed)

From trajectories, we improve the system:

```
┌─────────────────────────────────────────────────────────────────┐
│                    OUTER LOOP (System Improvement)              │
│                                                                 │
│   Session Logs  +  Rewards  →  Analysis  →  Better Policy      │
│                                                                 │
│   Tune: prompts, model selection, tool sets, task specs        │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                    INNER LOOP (Task Execution)                  │
│                                                                 │
│              step(state, task) → (state', result)              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**This is the key:** Every step is a data point for making the next step better.

## Human-on-Loop (Future)

Humans don't block the loop. Agents work continuously.

When an agent needs human input:
1. Queue the request (question, assumption, approval)
2. Return `Blocked`
3. Move to next task

Humans review asynchronously:
- Batch process queue items
- Zero wait latency between items
- Answers unblock tasks for future steps

*Not in v1 — human interaction is reviewing session branches and improving task specs between runs.*

## Design Principles

### 1. The Step is Atomic

One step = one task attempt. Complete isolation:
- Fresh worktree
- Fresh context (no accumulated state across tasks)
- Clean verification
- Clear outcome

### 2. Controlled Environment

We control everything going into the step:
- Isolated worktree (agent can't escape working directory)
- Project-level settings loaded (CLAUDE.md, skills, agents)
- No global/user-level ambient context
- Measured context (token tracking from first message)
- Agent has self-awareness of context usage

### 3. Observable Everything

Full visibility into the step:
- Every Claude message logged
- Every tool call and result logged
- Every rloop lifecycle event logged
- Token counts accumulated per message
- Context percentage tracked in real-time

### 4. Rewards Drive Improvement

We don't just run steps, we learn from them:
- Successful patterns → reinforce
- Failure modes → analyze and fix
- Task spec tuning based on outcomes
- Model selection based on task type success rates

## Success Criteria

1. **Step is solid** - Consistent, reproducible, observable
2. **Rewards are clear** - Success/Failed, no ambiguity
3. **Learning is possible** - Full trajectories logged for analysis
4. **Tasks are replayable** - Same specs can be run multiple times to compare results
5. **System improves** - Better task specs and configs from trajectory analysis

## Beyond v1

Once the core loop is solid:
- Parallel DAG execution (concurrent independent tasks)
- Human queue (ask_question, log_assumption, propose_task)
- Task creation by agents (discovered work becomes new tasks)
- Daemon mode (long-running, concurrent sessions)
- Community: weekly features, streams, blog posts
- Enable others to fork and build their own loops
