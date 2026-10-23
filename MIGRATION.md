# Migration to the Rust engine

The memory engine and every access, privacy, scope, injection, abstention, and session policy now run in Rust. The canonical data remains Markdown. SQLite remains a derived index.

## Module map

| Former implementation | Rust implementation | Contract moved |
|---|---|---|
| memory_fabric.py | rust/src/main.rs plus rust/src/db.rs, index.rs, retrieval.rs, propose.rs, policy.rs, and the remaining engine modules | CLI and JSON-lines dispatch, indexing, retrieval, proposals, migration, governance, and persistence |
| adapters/mcp.py | MCP stdio server in rust/src/main.rs | JSON-RPC protocol, tool schemas, annotations, prompts, parameter coercion, and stable responses |
| adapters/t3_code.py | T3 formatting and resume helpers in rust/src/extension.rs | Project slug and instruction rendering |
| remote-policy.ts | rust/src/extension.rs and rust/src/policy.rs | Provider allow/read/scrub/write gates; remote access defaults to scrubbed context and propose-only writes |
| scrub.ts | rust/src/extension.rs | Transcript tracking, vault and Apple Notes filtering, fail-closed overflow, fingerprinting, and reminder cadence |
| injection-precision.ts and abstention.ts | rust/src/extension.rs | Content-word selection, injection threshold, filename evidence, search abstention, and preserved engine reasons |
| memory-scope.ts | rust/src/extension.rs and rust/src/index.rs | Tier classification, project-note matching, per-project handoffs, and fail-closed provenance |
| injection.ts | rust/src/extension.rs | Injection accounting and prefix-change tracking |
| session-cost.ts | rust/src/extension.rs | Token accounting, notices, stale handoff checks, and bounded resume pointers |
| engine-health.ts | rust/src/extension.rs | Breaker, respawn budget, refusal accounting, and operator guidance |
| Python tests and Rust/Python parity harness | rust/src/tests.rs and rust/tests/protocol_conformance.rs | Engine behavior, migration, isolation, availability, adapters, protocols, writes, and revert-point assertions |
| TypeScript policy tests | Rust unit tests in rust/src/extension.rs | Former pure policy decision tables and edge cases |

All Python engine, adapter, test, parity, and benchmark-harness files are removed. The historical benchmark findings are retained in bench/evidence; the retired harness is described in bench/README.md. Evidence without retained raw data is labeled non-reproducible instead of being presented as a current measurement.

## Why index.ts remains

Pi loads an extension from index.ts, so JavaScript or TypeScript is required at the host boundary. The extension contains tool names and schemas required by Pi, event registration, JSON serialization, subprocess management, and response rendering. It does not decide whether a provider may read or write, what context to scrub, what to inject, whether to abstain, which memory tier to select, or how to account for session cost. Those decisions are Rust operations documented in docs/RUST-OPS.md.

The package has no npm runtime dependencies and no npm-based test framework. Pi supplies its extension API and TypeBox at runtime. The load check exercises the actual entry point with host API stubs.

## Compatibility notes

- PI_MEMORY_REMOTE_FULL_ACCESS is now opt-in. An allowlisted remote provider can read and propose by default; only an explicit positive value grants writes.
- Outbound transcript scrubbing now applies to allowlisted non-loopback providers by default. PI_MEMORY_REMOTE_CONTEXT_SCRUB=0 is an explicit operator opt-out.
- Init initializes the Markdown vault as a Git repository or fails loudly. Proposals remain review-only until applied.
- Messages cross the extension boundary as JSON values. A TypeScript test injected a throwing fingerprint callback; the Rust bridge cannot receive such a callback, so that artificial failure mode is replaced by deterministic fingerprints over every accepted JSON value.
- The version used by the runtime and installer comes from Cargo.toml. Historical release headings remain historical records.

## Test migration

The Rust test count moved from 26 to 98: 66 unit tests and 32 integration tests. The former TypeScript suites contained 82 test cases; their decision tables and edge assertions now run in Rust. The Python engine and adapter suites contained 34 named tests, with four additional procedural harnesses for availability, isolation, migration, and revert points; those contracts are exercised by the Rust integration suite. cargo test --all is the count source. No legacy suite runs in parallel with the shipped engine.
