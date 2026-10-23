# Archived implementation plan

This plan described an earlier dual-engine port sequence and a retrieval experiment plan. It is superseded by the current status in ROADMAP.md.

The engine and policy port is implemented in Rust. Python runtime, Python tests, parity harness, and Python-only benchmark scripts have been removed. The Pi extension remains TypeScript because the host loads index.ts; that file is transport only. See MIGRATION.md for the module map and docs/RUST-OPS.md for the current operation contract.

Recorded ranking experiment values remain in bench/evidence/ordering_ab_results.json. Memory-injection task success remains unmeasured. Do not use estimates from this archived plan as results.
