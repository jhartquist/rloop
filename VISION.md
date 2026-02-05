# rloop Vision

## Goal

Build the most powerful step function, then run it in loops to get work done.

Secondary goal: educate. Break everything into small chunks so anyone can learn how it's built and modify it for their own needs.

## The Step is Everything

## Core Insight

If we perfect the step function, the loop is trivial:

```
while tasks_remain() {
    step()
}
```

All complexity lives in `step()`. Everything else is orchestration.

## The Step

A step is a single atomic unit of work:

```
step(state, task) → (state', result)
```

**Inputs:**
- `state` - codebase, task queue, context
- `task` - what to accomplish

**Outputs:**
- `state'` - modified codebase, updated task status
- `result` - Success | Failed | Blocked

**What happens inside:**
1. Set up isolated environment (worktree)
2. Agent works (Claude with controlled tools)
3. Verify (tests, build)
4. Transition (commit, maybe merge)
5. Return result

## MDP Formulation

rloop is a Markov Decision Process:

```
MDP = (S, A, T, R, π)

S = State space (codebase × task_state × context)
A = Action space (a step execution)
T = Transition function: S × A → S'
R = Reward function: S × A → {Success, Failed, Blocked}
π = Policy: task → step_config
```

### State (S)

Everything observable before a step:
- Codebase (files, git history)
- Task state (from `br` - ready, blocked, done)
- Human queue state (from `hq` - pending questions, assumptions)
- Previous attempts (session logs)

### Action (A)

A step execution. Within the step, the agent takes many micro-actions (edits, tool calls), but from the MDP's view, the whole step is one action.

### Transition (T)

Deterministic given the step outcome:
- Success → task closed, code merged, state updated
- Failed → task remains, logs captured, retry possible
- Blocked → task blocked on human input

### Reward (R)

Sparse signal at end of step:
- `Success` - tests pass, task complete
- `Failed` - verification failed, agent gave up
- `Blocked` - needs human input (question, assumption review)

### Policy (π)

How we configure each step:

```rust
struct StepConfig {
    model: Model,              // haiku, sonnet, opus
    system_prompt: String,     // task framing
    tools: Vec<Tool>,          // allowed tools (explicit allowlist)
    max_turns: u32,            // budget
    verification: Vec<String>, // commands to run
}
```

The policy maps tasks to configs. Initially hardcoded, later learned.

## The Learning Loop

Every step produces a trajectory:

```
τ = [(s₀, a₀), (s₁, a₁), ..., (sₙ, aₙ), reward]
```

We log everything:
- Full session events (Claude messages, tool calls, results)
- Config used (model, prompt, tools)
- Metrics (tokens, duration, tool call counts)
- Outcome (Success/Failed/Blocked)

From trajectories, we improve the system:

```
┌─────────────────────────────────────────────────────────────────┐
│                    OUTER LOOP (System Improvement)              │
│                                                                 │
│   Session Logs  +  Rewards  →  Analysis  →  Better Policy      │
│                                                                 │
│   Tune: prompts, model selection, tool sets, process           │
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

## Human-on-Loop

Humans don't block the loop. Agents work continuously.

When an agent needs human input:
1. Queue the request (question, assumption, approval)
2. Return `Blocked`
3. Move to next task

Humans review asynchronously:
- Batch process queue items
- Zero wait latency between items
- Answers unblock tasks for future steps

## Design Principles

### 1. The Step is Atomic

One step = one task attempt. Complete isolation:
- Fresh worktree
- Fresh context (no accumulated state)
- Clean verification
- Clear outcome

### 2. 100% Control

We control everything going into the step:
- Empty starting directory (no ambient CLAUDE.md or skills)
- Explicit tool allowlist (disable all by default)
- Measured context (token tracking from first message)
- Sandboxed filesystem (can't escape working directory)

### 3. Observable Everything

Full visibility into the step:
- Every Claude message logged
- Every tool call and result logged
- Token counts accumulated per message
- Context percentage tracked in real-time

### 4. Rewards Drive Improvement

We don't just run steps, we learn from them:
- Successful patterns → reinforce
- Failure modes → analyze and fix
- Prompt tuning based on outcomes
- Model selection based on task type success rates

## Success Criteria

1. **Step is solid** - Consistent, reproducible, observable
2. **Rewards are clear** - Success/Failed/Blocked, no ambiguity
3. **Learning is possible** - Full trajectories logged for analysis
4. **Humans aren't bottlenecks** - Async queue, batch review
5. **System improves** - Measurable gains from trajectory analysis

## Beyond the Code

Once rstep is solid:
- Build rloop (parallel orchestration, merge queue, human queue)
- Community: weekly features, streams, blog posts
- Enable others to fork and build their own loops
