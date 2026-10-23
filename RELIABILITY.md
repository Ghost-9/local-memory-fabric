# Reliability and recovery

## Source of truth

Markdown notes in the configured vault are canonical. SQLite/FTS and caches are derived and can be rebuilt. Reindexing must not modify note contents. Init initializes Git before enabling canonical writes; if it cannot establish a revert point, it fails.

## Security gates

Rust keeps provider access, transcript scrubbing, and write authorization as separate decisions. An unknown provider or endpoint is denied. Allowlisted remote providers receive scrubbed outbound context by default and are propose-only. The explicit context-scrub opt-out and full-write opt-in are independent settings.

A scrub-tracking overflow withholds uncertain tool results. A broken or unavailable Rust engine blocks policy-sensitive operations; the extension does not fall back to Python or make a local allow decision.

## Recovery

- Use doctor and verify to diagnose before changing the index.
- Rebuild the derived index from Markdown after a confirmed index problem.
- Review proposals before applying them.
- Use the Git history in the vault as the canonical write rollback point.
- Install/update leaves memory in place; uninstall keeps the state directory unless purge is explicitly requested.
- Run ./test.sh before committing engine or policy changes.

## Evidence

This repository does not claim a task-success benefit for memory injection. It also does not promote historical latency results whose original per-run records are absent. See bench/evidence for recorded and unmeasured findings.
