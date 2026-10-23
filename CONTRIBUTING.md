# Contributing

The Rust binary is the sole implementation of memory and policy behavior. The TypeScript Pi extension is a host-required transport adapter.

## Requirements

- Rust toolchain available through Cargo.
- Node available for the Pi extension load check.
- No Python interpreter and no npm install are required.

## Workflow

1. Make the Rust behavior change and add or update a Rust unit or integration test.
2. Keep the extension limited to registering Pi tools and hooks, JSON transport, and rendering.
3. Run ./test.sh. It is the repository gate and has no silent skip.
4. Update README, CHANGELOG, ROADMAP, or MIGRATION when behavior or architecture changes.
5. Commit only after every gate is green.

For security-sensitive changes, retain separate access, context-scrub, and write checks. Unknown providers, malformed endpoints, untrusted provenance, secret-shaped payloads, and scrub overflow must fail closed.

Performance and quality claims must cite retained records in bench/evidence. The benchmark harness that depended on Python has been retired; historical results without raw records are labeled non-reproducible.
