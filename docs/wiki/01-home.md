# Engram

Engram is a local-first memory engine for coding agents. Markdown is canonical; the SQLite index is disposable. Rust owns memory behavior and policy. Pi still loads index.ts, which is transport only.

## Working rules

- Run ./test.sh and require every gate to pass.
- Keep provider access, transcript scrubbing, and writes as separate Rust decisions.
- Fail closed on unknown providers, unsafe paths, missing provenance, and scrub overflow.
- Keep private vault content and credentials out of this public repository.
- Treat unmeasured claims as unmeasured.

## Where things live

| Path | Purpose |
|---|---|
| rust/src | Engine, policy, MCP protocol, and extension operations |
| rust/tests/protocol_conformance.rs | Protocol, migration, availability, isolation, and write contracts |
| index.ts | Pi host adapter: register, serialize, call Rust, render |
| docs/RUST-OPS.md | Rust operation contract |
| bench/evidence | Retained results and unmeasured work |
| test.sh | Sole gate |

See README.md for setup, MIGRATION.md for the port map, and ROADMAP.md for remaining work.
