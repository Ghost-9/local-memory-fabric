# Wiki content

These pages are maintained in docs/wiki. README.md is the product and setup reference, ROADMAP.md tracks current work, CHANGELOG.md records changes, and docs/RUST-OPS.md documents the Rust operation contract.

| Page | Purpose |
|---|---|
| 01-home.md | Architecture and repository map |
| 02-access-gates.md | Provider, transcript, and write policy |
| 03-cache-stability.md | Deterministic response contract and measurement limits |
| 04-project-memory-lifecycle.md | Project notes, proposals, and handoffs |
| 04-roadmap-index.md | Link to current roadmap |
| 05-measurements.md | Retained and unmeasured evidence |

The authoritative behavior checks are Rust unit and integration tests run by ./test.sh. If a wiki page disagrees with Rust behavior and its tests, update the page.
