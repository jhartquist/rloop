# rloop (crate)

The Rust crate for [rloop](../README.md). See the workspace README for what
rloop is, how to install it, and how to use it.

## Install from source

```bash
cargo install --path .
```

## Examples

```bash
cargo run --example 00-minimal-query
cargo run --example 01-token-tracking
```

## Coding conventions

See [CLAUDE.md](CLAUDE.md). Pedantic clippy is required; run `mise run ci`
before commits.
