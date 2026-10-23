# Historical audit report

This document previously described the repository as a dual-engine release and included security, performance, and retrieval claims that were not consistently backed by retained artifacts. It is archived as a historical record, not a current audit or assurance statement.

The current implementation is a Rust engine with a Pi-required TypeScript transport. Read README.md for the active architecture, MIGRATION.md for the port map, and docs/RUST-OPS.md for the policy contract. Current tests are run by ./test.sh.

No metric from this historical report should be repeated as a current measurement unless it is present in bench/evidence with enough retained data to check it. Historical latency notes with missing raw runs are explicitly marked in bench/evidence/retired-latency-findings.md. Task-success benefit from memory injection is unmeasured.
