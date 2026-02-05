# rstep

Single-step task executor. Run one step, get a result.

## Build Method

Chronological examples in `examples/`. Each builds on the last.

### Commits

One logical change per commit. Add dependencies just-in-time.

```
ex-XX: description (N lines)
deps: add <crate>
fix: what was fixed
refactor: extract to lib
test: what's tested
```

### Fixing Earlier Examples

```bash
git rebase -i <commit>^
# Mark "edit", fix, amend, continue
git push --force-with-lease
```

## Principles

- **Bare minimum**: Empty temp dir, no ambient context, explicit tool allowlist
- **Track tokens**: Accumulate from every stream message
- **Sandbox**: Claude can only access working directory
- **Structured output**: Final message is JSON StepResult

## Structure

```
rstep/
├── src/lib.rs        # Grows as we extract from examples
├── examples/         # 00-, 01-, 02-, ...
├── templates/
│   ├── bug-adder/    # Fix a-b → a+b
│   └── empty/
├── fixtures/recorded/
└── tests/
```

## Run

```bash
cargo run --example 00-minimal-query
cargo test
```
