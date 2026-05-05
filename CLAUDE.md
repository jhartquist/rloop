# rloop

DAG-based task executor. Run tasks in isolated worktrees, verify results, merge work.

## Coding Style

### Rust Conventions

- **Pedantic clippy.** `#![warn(clippy::pedantic)]` in lib.rs. The CLI lint command passes `-W clippy::pedantic` so all targets (tests, examples, binary) are also checked. Fix all warnings.
- **Functional style.** Prefer returning values over mutation. Use `Result` chains, iterators, combinators. Minimize `&mut self` — prefer transformations that return new values.
- **Deep modules, simple interfaces.** Each module exposes 1-3 public functions/types. Hide complexity behind clean APIs. Callers shouldn't need to know internals.
- **Descriptive names.** Functions and variables should read like prose. No abbreviations unless universally understood (`id`, `dir`, `config`). No hungarian notation.
- **Terse comments.** Comment *why*, not *what*. No comments that restate the code. A well-named function needs no comment. Add comments for non-obvious decisions, tradeoffs, or workarounds.
- **No dead code.** Don't leave commented-out code, unused imports, or `#[allow(dead_code)]`. If it's not used, delete it.
- **Error handling.** Use `anyhow` for application errors. Use `thiserror` for library error types that callers match on. No `.unwrap()` outside tests.

### Dependencies

**Add dependencies one at a time, only when first used.**

```
deps: add tokio for async runtime
deps: add serde for task file parsing
```

Don't batch dependencies. Don't add them "in preparation" for later use.

### Development Workflow

Before every commit:

```bash
cargo fmt                                          # format code
cargo clippy --all-targets -- -D warnings -W clippy::pedantic  # lint (must pass clean)
```

Run all checks at once from the repo root:

```bash
mise run ci    # runs fmt check + clippy + tests
```

### Commits

One logical change per commit. Small commits are better than big ones. Run `cargo fmt` before every commit. Commit regularly — after each logical change, not in large batches.

```
feat: add task file parsing
feat: add DAG resolution with topological sort
fix: handle circular dependency detection
refactor: extract worktree lifecycle to git module
test: add DAG resolution edge cases
deps: add serde for task file parsing
```

## Module Layout

```
rloop/src/
├── lib.rs          # Public API, clippy config
├── task.rs         # Task parsing, DAG resolution
├── step.rs         # Single step execution (agent + verify + retry)
├── git.rs          # Worktree lifecycle, branching, merging
├── session.rs      # Session orchestration
├── log.rs          # Event logging (JSONL + terminal output)
├── tools.rs        # MCP tool definitions (complete)
├── config.rs       # Config loading from .rloop/config.toml
└── main.rs         # CLI entry point
```

## Testing

- **Unit tests** in each module (`#[cfg(test)] mod tests`).
- **Integration tests** in `tests/` for cross-module flows.
- **Examples** in `examples/` for smoke tests and demos.
- Tests use real file I/O with `tempfile` crate. No mocks unless unavoidable.

```bash
cargo clippy --all-targets -- -D warnings -W clippy::pedantic    # lint
cargo test                              # all tests
cargo run --example 01-parse-task       # smoke test
```

## Run

```bash
cargo clippy --all-targets -- -D warnings -W clippy::pedantic
cargo test
cargo run --example 01-parse-task
```
