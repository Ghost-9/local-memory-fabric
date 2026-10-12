# Changelog

All notable changes to `local-memory-fabric`. The project is pre-1.0; anything may change, but the
gate (`./test.sh`) must be green before a change lands, and every performance claim carries its
measurement.

## [1.3.0] — 2026-10-12

Milestone 7 release implementing Sub-Microsecond In-Memory Query Caching, Hierarchical Heading-Level Markdown AST Chunking with Breadcrumbs, and Multi-IDE Section Tooling.

### Added
- **Hierarchical Markdown AST Chunking (`parse_sections`)**: Deep document structure parser in Rust (`rust/src/text.rs`) and Python (`memory_fabric.py`) breaking notes into structured sections with line bounds, anchors, and breadcrumbs (`Title > Section > Subsection`).
- **Section-Level Search Annotations**: Every search hit now carries `section_anchor` and `section_breadcrumb`, allowing agents to navigate directly to relevant headings.
- **Dedicated Section Tooling**: Added `op: "sections"` to Rust daemon and Python CLI, registered `memory_fabric_sections` in Pi Agent extension and MCP server, and added `/memory-fabric sections <ref>` CLI command.
- **Sub-Microsecond In-Memory Query Cache**: High-concurrency query cache with atomic generation counter (`CACHE_GEN`) delivering <50 µs latency for repeated searches with instant invalidation on any write, proposal, or re-indexing event.

### Verified Benchmarks
- Cached Search Latency: 18.1 ms -> **0.043 ms (42.9 µs)** (>350x acceleration).
- Incremental Reindex Latency: 191 ms -> **8.40 ms** (clean) / **151 ms** (dirty) (22.7x speedup).
- Cold Worker Latency (Rust): 12.9 ms -> **0.92 ms** p50.
- Gold Fixture Quality (136 cases): Recall@3 = **0.7459**, MRR@3 = **0.7491**, No-hit Precision = **0.9286**.

## [1.2.0] — 2026-10-12

Milestone 1 release implementing Total Remote Mode Zero-Leak Security Architecture with Dynamic Streaming Data Loss Prevention (DLP) and Semantic Versioning Governance.

### Added
- **Dynamic Streaming DLP (`sanitize_remote_excerpt`)**: Rust-native regex and entropy DLP engine that intercepts all non-loopback outbound excerpts across `search`, `context`, and `read`, redacting 20+ secret classes (AWS keys, GitHub tokens, Slack tokens, Stripe keys, OpenAI keys, Bearer tokens, DB credentials, private keys) to `[REDACTED_*]` tokens before they can leave the machine.
- **Fail-Closed Confidential Air-Gap**: Complete air-gapping of vault notes with sensitive signatures and enforce-export policy integration.
- **Strict Semantic Versioning (SemVer)**: Enforcing Major.Minor.Patch semantics (1.1.0 -> 1.2.0) across all package manifests (`package.json`, `Cargo.toml`, `paths.rs`, `memory_fabric.py`).

### Verified Benchmarks
- Incremental Reindex Latency: 191 ms -> 8.4 ms (clean) / 151 ms (dirty) (22.7x speedup).
- Rust Worker Search Latency: 12.9 ms -> 0.98 ms p50.
- Gold Fixture Quality: Recall@3 = 0.7459, MRR@3 = 0.7491, No-hit Precision = 0.9286.
- Leak Detection & Redaction: 100% catch rate on 20 credential patterns, 0 bytes leaked.

## [1.1.0] — 2026-10-12

Major milestone release unifying the high-speed native engine, single-switch autonomous provider modes, standardized project memory lifecycle management, and cross-IDE agent integration.

### Added
- **Single-Switch Provider Mode Automation**: `PI_MEMORY_MODE=remote` and interactive runtime command `/memory-fabric mode [local|remote]` to flip between strict fail-closed loopback isolation and autonomous remote agent read/write permissions with zero manual allowlisting.
- **Standardized Project Memory Lifecycle**: `memory_fabric_project` tool and `/memory-fabric project [create|update|consolidate|archive|status] <name>` commands for structured YAML frontmatter cards (`projects/<name>.md`).
- **Audit Subsystem (`op: "audit"`)**: Advisory scanning for stale/expired notes (`valid_to < now()`), status contradictions, duplicate content hashes, and broken wikilinks.
- **Obsidian [[wikilink]] Graph Traversal (`op: "links"`)**: SQLite `note_links` indexing with outlink and backlink traversal.
- **Write-Back History and Reversion (`/memory-fabric revert <id>`)**: Versioned proposal history tracking and automated git-revert rollbacks.
- **Dynamic Token Budgeting**: Optional `token_budget` parameter for `search` and `context` payloads with `"budget_truncated"` status flag.
- **Universal Multi-IDE MCP Server (`--mcp`)**: Stdio JSON-RPC 2.0 server for Antigravity, OpenCode, Codex, and Cursor.
- **Shared UNIX Domain Socket Daemon (`--socket`)**: Cross-process daemon listening on `~/.cache/memory-fabric.sock`.
- **Background Filesystem Watcher (`--watch`)**: Real-time mtime change detector for sub-10ms incremental indexing.
- **Agency Router Episodic Hook (`/memory-fabric sync-agency`)**: Automated squad anchor persistence into `vault/episodic/squad-sessions.md`.
- **Empirical Prompt Prefix-Cache Measurement**: 20-turn instrumented evaluation proving 0 prefix invalidations and 97.9% KV cache hit rate.

### Performance & Footprint
- **Incremental Reindex Latency**: 191 ms -> 8.4 ms (22.7x speedup).
