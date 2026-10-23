# Porting status

The engine and policy migration is complete in this branch. The active architecture and exact module map are documented in MIGRATION.md.

Rust owns the memory engine, MCP server, provider policy, transcript scrubbing, DLP, injection and abstention, memory scope, handoff handling, session cost, and health policy. The Pi host still requires index.ts, so that file remains as transport only.

Python runtime, Python tests, adapter code, parity harnesses, and Python benchmark scripts have been removed. Rust unit and integration tests are the regression contract. Historical benchmark findings remain under bench/evidence with their reproducibility limits stated.
