# Build Methodology

We build rstep through **chronological examples** - a series of incremental programs that teach the implementation step by step.

## Git Workflow

### Structure

Linear commits on main, each example is 1-3 commits:

```
main: [ex-00] → [ex-01] → [ex-02] → ... → [ex-N]
```

The final example (ex-N) demonstrates the complete, current crate.

### Commit Messages

```
ex-00: minimal Claude query

- Call SDK with bare minimum config
- Empty temp dir, no tools, no CLAUDE.md
- Print response and token counts
- 47 lines added
```

Always include:
- Example number prefix: `ex-XX:`
- What it adds (brief)
- Lines added (for teaching clarity)

### Fixing Earlier Examples

When you find a bug in example 20 while working on example 40:

```bash
# 1. Find the commit to fix
git log --oneline
# abc123 ex-40: current work
# ...
# def456 ex-20: token tracking  <-- this one has the bug

# 2. Start interactive rebase from BEFORE the buggy commit
git rebase -i def456^

# 3. In the editor, change "pick" to "edit" for the ex-20 commit
# pick def456 ex-20: token tracking
#   becomes:
# edit def456 ex-20: token tracking

# 4. Git stops at that commit. Make your fix.
# Edit the files...
git add -A
git commit --amend --no-edit  # or edit message if needed

# 5. Continue the rebase - all later commits get the fix
git rebase --continue

# 6. If conflicts arise, fix them and continue
# git add <fixed-files>
# git rebase --continue
```

### Important Notes

- **History rewrites are expected** - This is teaching material, not production code
- **Force push required** after rebase: `git push --force-with-lease`
- **Don't rebase** if others are actively working on the same branch
- **Test after rebase** - Run examples to make sure they still work

### Adding a New Example

```bash
# 1. Create the example file
touch examples/25-new-feature.rs

# 2. Implement it (building on previous examples)

# 3. Add tests if applicable
touch tests/unit/new_feature_test.rs

# 4. Commit with proper message
git add examples/25-new-feature.rs tests/unit/new_feature_test.rs
git commit -m "ex-25: add new feature

- Description of what this adds
- Key changes
- 52 lines added"

# 5. If extracting to lib, do a second commit
git add src/new_feature.rs examples/25-new-feature.rs
git commit -m "ex-25: extract new_feature to lib

- Move implementation to src/new_feature.rs
- Example now uses library module
- Net: +30 to lib, -40 from example"
```

### Recovery

If a rebase goes wrong:

```bash
# Abort and return to pre-rebase state
git rebase --abort

# Or if you already finished but want to undo:
git reflog  # Find the commit before rebase
git reset --hard HEAD@{N}  # N = steps back in reflog
```

## Philosophy

### Examples as Documentation

Each example is a complete, runnable program that demonstrates one concept:

```
examples/
├── 00-minimal-query.rs      # Just call Claude, print response
├── 01-token-tracking.rs     # Track tokens per message
├── 02-disable-tools.rs      # Explicit tool allowlist
├── 03-custom-tools.rs       # Add our own MCP tools
├── 04-structured-output.rs  # Require JSON result format
├── 05-temp-directory.rs     # Sandboxed working directory
├── 06-security-tests.rs     # Verify sandbox boundaries
├── 07-br-integration.rs     # Get task from beads_rust
├── 08-worktree-setup.rs     # Create git worktree
├── 09-verification.rs       # Run tests, check results
├── 10-merge-flow.rs         # Squash merge on success
├── 11-session-logging.rs    # Log all events to JSONL
├── 12-recording.rs          # Record Claude responses
├── 13-replay.rs             # Replay from fixtures
└── ...
```

### Two-Step Pattern

Each major feature goes through two commits:

1. **Raw example** - New functionality demonstrated inline
2. **Extract to lib** - Move reusable code to `src/lib.rs`, example becomes thin wrapper

```
Commit 1: "feat(examples): 05 - sandboxed temp directory"
  - examples/05-temp-directory.rs (full implementation inline)

Commit 2: "refactor: extract sandbox module to lib"
  - src/sandbox.rs (reusable module)
  - examples/05-temp-directory.rs (now just uses lib)
```

### Bug Fixes Go in Place

If we find a bug in example 05 while working on example 12:

- Fix it in the logical place (example 05)
- The fix appears in the chronological history as if we got it right the first time
- No separate "fix" commits cluttering the history

This creates a **clean teaching narrative** - as if we built it perfectly from the start.

## Project Structure

```
rstep/
├── Cargo.toml
├── CLAUDE.md                    # Build conventions for this project
├── src/
│   └── lib.rs                   # Grows as we extract from examples
├── examples/
│   ├── 00-minimal-query.rs
│   ├── 01-token-tracking.rs
│   └── ...
├── templates/
│   ├── hello-rust/              # Minimal Rust project
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   └── empty/                   # Truly empty directory
├── fixtures/
│   └── recorded/                # Recorded Claude sessions
└── tests/
    ├── unit/                    # Unit tests for lib modules
    └── integration/             # Full flow tests
```

## Test Strategy

### Examples Demonstrate

Examples show how things work. Run them to see output:

```bash
cargo run --example 05-temp-directory
```

### Tests Prove Correctness

Tests exhaust all cases:

```rust
// tests/integration/sandbox_test.rs

#[test]
fn test_can_read_in_sandbox() { ... }

#[test]
fn test_cannot_read_parent_directory() { ... }

#[test]
fn test_cannot_read_home_directory() { ... }

#[test]
fn test_cannot_read_sibling_temp_directory() { ... }

#[test]
fn test_cannot_write_outside_sandbox() { ... }
```

### Recording for Determinism

Integration tests use recorded sessions:

1. **First run** - Record real Claude responses to fixture
2. **Subsequent runs** - Replay from fixture (fast, deterministic, free)
3. **Optional** - `--real` flag to run against live API

```rust
#[test]
fn test_step_success() {
    let agent = RecordedAgent::from_file("fixtures/recorded/success.json");
    let result = run_step(agent, task).await;
    assert!(matches!(result, StepResult::Success { .. }));
}
```

## Naming Convention

Examples use zero-padded numbers:

```
00-name.rs
01-name.rs
...
09-name.rs
10-name.rs
...
99-name.rs
```

If we exceed 99, rename all to three digits: `000-`, `001-`, etc.

## Commit Strategy

### Feature Commits

```
feat(examples): 07 - br integration

Add example showing how to get tasks from beads_rust
and update their status.
```

### Extraction Commits

```
refactor: extract br module to lib

Move br CLI wrapper to src/br.rs. Example 07 now
uses the library module.
```

### Test Commits

```
test: add br integration tests

Exhaustive tests for br CLI wrapper including
error cases and edge conditions.
```

## Top-Down Development

Within each example, we follow top-down development:

1. **Types first** - Define the data structures
2. **Function signatures** - Define the interface with `todo!()`
3. **Tests** - Write tests against the interface
4. **Implementation** - Fill in the `todo!()`s

```rust
// Step 1: Types
struct StepResult { ... }

// Step 2: Signatures
fn run_step(task: &Task) -> StepResult {
    todo!()
}

// Step 3: Tests
#[test]
fn test_run_step_success() {
    let result = run_step(&task);
    assert!(matches!(result, StepResult::Success { .. }));
}

// Step 4: Implementation
fn run_step(task: &Task) -> StepResult {
    // actual implementation
}
```

## Templates

Templates are minimal project scaffolds for testing:

### hello-rust

```
templates/hello-rust/
├── Cargo.toml
└── src/
    └── main.rs    # fn main() { println!("Hello!"); }
```

### empty

```
templates/empty/
└── .gitkeep       # Truly empty, for maximum control tests
```

Tests copy templates to temp directories:

```rust
#[test]
fn test_step_in_rust_project() {
    let temp = copy_template("hello-rust");
    init_beads(&temp);
    create_task(&temp, "Add a greeting function");
    let result = run_step(&temp);
    // ...
}
```

## Progress Tracking

We track which examples are complete:

| # | Example | Status | Lib Module |
|---|---------|--------|------------|
| 00 | minimal-query | TODO | - |
| 01 | token-tracking | TODO | metrics.rs |
| 02 | disable-tools | TODO | tools.rs |
| 03 | custom-tools | TODO | tools.rs |
| 04 | structured-output | TODO | output.rs |
| 05 | temp-directory | TODO | sandbox.rs |
| 06 | security-tests | TODO | sandbox.rs |
| 07 | br-integration | TODO | br.rs |
| 08 | worktree-setup | TODO | git.rs |
| 09 | verification | TODO | verify.rs |
| 10 | merge-flow | TODO | git.rs |
| 11 | session-logging | TODO | session.rs |
| 12 | recording | TODO | recording.rs |
| 13 | replay | TODO | recording.rs |

## Quick Reference

### Common Commands

```bash
# Run an example
cargo run --example 05-temp-directory

# Run all tests
cargo test

# Run specific test
cargo test test_sandbox_security

# See example list
ls examples/

# Check what the next example should be
ls examples/ | tail -1
```

### Git Cheatsheet

```bash
# Fix earlier example (e.g., ex-15)
git log --oneline                    # Find commit
git rebase -i <commit>^              # Start rebase
# Change "pick" to "edit", save
# Make fixes
git add -A && git commit --amend
git rebase --continue                # Apply to all later commits

# Abort if things go wrong
git rebase --abort

# After successful rebase
git push --force-with-lease
```

### New Example Checklist

- [ ] File: `examples/XX-name.rs`
- [ ] Builds on previous example
- [ ] Clear comments explaining what's new
- [ ] Tests added if applicable
- [ ] Commit message includes line count
- [ ] Extract to lib in follow-up commit (if reusable)
