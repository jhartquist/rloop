# rloop-planning

Planning documents for rloop - an AI agent orchestrator built on the MDP framework.

## Quick Start

1. **VISION.md** - The step as atomic unit, MDP formulation, learning loop
2. **RSTEP.md** - The step executor design
3. **BUILD.md** - Chronological examples methodology

## The Core Idea

**The step is everything.**

```
step(state, task) → (state', result)
```

If we perfect the step, the loop is trivial:

```rust
while tasks_remain() {
    step()
}
```

## MDP Framing

```
S = State (codebase × task_state × context)
A = Action (a step execution)
T = Transition (deterministic given outcome)
R = Reward (Success | Failed | Blocked)
π = Policy (how we configure each step)
```

Every step is a data point. We learn from trajectories to improve the system.

## The Tool Stack

```
rstep    single step executor    (this is what we're building)
br       beads_rust              task tracking (external)
hq       human queue             human input (future)
rloop    orchestrator            parallel steps + merge queue (future)
```

## Build Order

1. **rstep** - Single task executor via chronological examples
2. **hq** - Human queue for questions/assumptions (when needed)
3. **rloop** - Parallel orchestration with merge queue

## Files

```
rloop-planning/
├── README.md       # This file
├── VISION.md       # MDP framing, learning loop, principles
├── RSTEP.md        # Step executor design
├── BUILD.md        # Chronological examples methodology
└── archive/        # Previous planning docs (for reference)
```

## Archive

Previous planning documents are in `archive/`. They informed the current design but we've simplified and refocused on the step as the atomic unit.
