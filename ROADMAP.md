# Roadmap

This file describes the current implementation and remaining evidence work. The package version is sourced from rust/Cargo.toml; historical milestone versions belong to the changelog and commit history.

## Shipped in this migration

- Rust is the only memory and policy engine.
- The Pi extension is transport-only and has no npm runtime dependencies.
- MCP protocol handling, parameter normalization, prompts, tool annotations, and deterministic JSON responses run in Rust.
- Provider access, remote context scrubbing, propose-only remote writes, DLP, injection, abstention, tiers, handoffs, session cost, and health operations run in Rust.
- Init prepares a Git-backed Markdown vault or fails; init and doctor explain that an empty vault has no memories to return.
- The installer builds from Cargo when available and rejects a downloaded binary whose version differs from Cargo.toml.
- Python runtime, tests, parity checks, and Python-only benchmark harnesses are removed.
- Rust formatting, Clippy, unit tests, integration tests, protocol conformance, installer contracts, and extension loading share one gate.

## Remaining work

- Measure whether memory context improves task success and at what token cost. No such result is claimed; the pre-registration and current status are in bench/evidence/README.md.
- Retain raw benchmark runs before publishing any future latency or retrieval-quality claim.
- Publish release binaries only from the version declared in Cargo.toml and attach checksums generated from those artifacts.
- Expand protocol conformance when the MCP or Pi host contract changes.

## Decision rules

- Markdown stays canonical and the index stays disposable.
- Keep the access, scrub, and write gates separate.
- Remote providers stay propose-only unless an operator opts in.
- Do not relax fail-closed behavior to satisfy a test or improve convenience.
- Unmeasured remains the correct status when evidence is missing.
