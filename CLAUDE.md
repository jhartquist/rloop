# rloop-planning

Planning docs for rstep and rloop.

## Working on rstep

The rstep crate is in `rstep/` (git subtree). See `rstep/CLAUDE.md` for crate-specific instructions.

### Dependencies

**Add dependencies one at a time, only when first used.**

Each dependency gets its own commit:
```
deps: add tokio for async runtime
deps: add tempfile for temp directories
deps: add serde_json for JSON output
```

Don't batch dependencies. Don't add them "in preparation" for later use.

### Commits

One logical change per commit. Small commits are better than big ones.

### Pushing to rstep repo

After commits in `rstep/`:
```bash
git subtree push --prefix=rstep git@github.com:USERNAME/rstep.git main
```

## Files

- `@VISION.md` - Goals, MDP framing, philosophy
- `@RSTEP.md` - Step executor design
- `@BUILD.md` - Chronological examples methodology
- `EXAMPLES.md` - Detailed plan for each example
- `rstep/` - The crate (subtree)
