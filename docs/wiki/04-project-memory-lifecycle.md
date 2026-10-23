# Project memory lifecycle

Project notes live under projects/ in the Markdown vault. Global notes remain eligible across projects. Other notes are episodic and are not automatically injected. Rust matches project-note slugs against the current working directory and rejects ambiguous or missing project provenance.

Project lifecycle actions are exposed through memory_fabric_project. Changes are stored as reviewable proposals outside the canonical vault. Inspect a proposal before applying it. Git history provides the revert point for applied changes; init creates the repository or fails.

Handoffs are stored per normalized project directory. A handoff without matching cwd provenance is not eligible for injection. The transcript path is included only when available.

The project and handoff operation contract is documented in docs/RUST-OPS.md and exercised by the protocol conformance tests.
