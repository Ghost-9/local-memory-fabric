# Agent and contributor instructions

This repository ships a Rust memory engine and a small TypeScript transport because Pi loads index.ts. The Markdown vault is canonical; the SQLite index is derived.

## Required workflow

- Work on a branch and keep the change scoped to this repository.
- Run ./test.sh after implementation. It is the sole gate.
- Do not commit or open a pull request while a gate is red or unverified.
- Do not add Python or npm runtime/test dependencies.
- Keep policy and behavior in Rust. index.ts may register schemas and hooks, serialize JSON, call the Rust process, and render results.
- For a security change, preserve the existing fail-closed contract and add a Rust test that would catch a regression.
- Never expose vault content or credentials in repository artifacts.
- Do not invent benchmark results. Mark work unmeasured unless a retained evidence record supports the claim.
- Do not modify the canonical vault while testing. Use temporary fixtures.

## Gate

test.sh runs cargo fmt --all --check, Clippy with warnings denied, cargo test --all, protocol and installer conformance, and the extension load check. It fails if Rust or Node is missing. A required check may not be silently skipped.

## Where to look

- rust/src contains the engine and policy.
- rust/tests/protocol_conformance.rs covers protocol, migration, availability, isolation, and write behavior.
- docs/RUST-OPS.md describes extension-facing Rust operations.
- MIGRATION.md maps legacy policy code to Rust.
- bench/evidence contains recorded findings and unmeasured work.
- CHANGELOG.md and ROADMAP.md describe current state.

Make commits only after the gate is green. Do not force-push or rewrite history.
