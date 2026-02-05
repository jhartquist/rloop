# rstep

Single-step task executor for AI agent loops.

## Usage

```bash
rstep              # Run next ready task
rstep bd-abc123    # Run specific task
rstep --no-merge   # Skip auto-merge
```

## Learning

Work through examples in order:

```bash
cargo run --example 00-minimal-query
cargo run --example 01-token-tracking
# etc.
```

## Install

```bash
cargo install --path .
```
