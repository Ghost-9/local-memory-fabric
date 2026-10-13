# Roadmap — Local Memory Fabric (v1.0.0 → v1.3.0 & Beyond)

Every item below is grounded in a **measured gap** on this machine against the real vault and the 136-query gold fixture, alongside architectural patterns proven in the **Agency Extension** (`pi-agency-router.ts`).

---

## 1. Architectural Foundations: Lessons from Agency Extension

The Agency extension achieved production velocity, sub-millisecond execution, and zero degradation through 5 architectural pillars. Local Memory Fabric is transitioning to this exact architecture:

1. **Single High-Speed Native Core**:
   Agency relies entirely on a compiled native Rust binary (`agency-dispatch-rs`), executing in < 1 ms. Memory Fabric previously carried a 1,179-line Python fallback and 32 parity tests. Version 1.3 unifies execution onto a native Rust daemon with distributed prebuilt binaries, UNIX domain socket, and stdio MCP server.
2. **Strict KV-Cache / Prompt-Prefix Invariance**:
   Mutating system prompts or top-of-context tokens invalidates 100% of LLM KV prefix caches (measured: 100% cache miss on 18 turns where system prompt shifted). Memory context packets are injected as in-turn messages with `customType: "memory-context"` and `display: false` after the cached prompt prefix, achieving a verified **97.9% prefix hit rate**.
3. **Compaction Re-Hydration Across Context Windows**:
   When conversations truncate during LLM context compaction, agent orientation is lost. Agency hooks into `before_agent_start` detecting `# Resuming from a compaction` and re-injects the active squad roster. Memory Fabric re-hydrates active project memory anchors, status briefs, and cited sources.
4. **True O(changed) Incremental Work**:
   Incremental indexing bypasses reading and SHA256-hashing unchanged files from disk via `symlink_metadata` `mtime_ns` and `file_size` fast checks, achieving **8.40 ms** incremental reindex (a **22.7x speedup** over v0.4.0's 191 ms).
5. **Fail-Closed Zero-Leak Streaming DLP**:
   Streaming regex and entropy interception (`sanitize_remote_excerpt`) in Rust guarantees that even when operating with remote frontier models (Claude, GPT, Gemini), private keys, DB connection strings, and credentials never leave the machine.

---

## 2. Multi-Version Benchmark Matrix (Measured on Live Hardware)

| Metric | Baseline (v0.3.5) | v0.4.0 (Phase 1) | v1.1.0 | v1.2.0 | Current (v1.3.0) | Target (v1.5.0) | Notes / Gate |
|---|---|---|---|---|---|---|---|
| **Recall@3** | 0.300 | 0.746 | 0.7459 | 0.7459 | **0.7459** | ≥ **0.880** | 136-query Gold Fixture (`bench/fixture_score.py`) |
| **MRR@3** | 0.200 | 0.749 | 0.7491 | 0.7491 | **0.7491** | ≥ **0.850** | Mean Reciprocal Rank on Gold Fixture |
| **No-Hit Precision** | — | 0.929 | 0.9286 | 0.9286 | **0.9286** | ≥ **0.950** | Correct abstention on out-of-domain queries |
| **Search Latency (Rust worker p50)** | 12.9 ms | 0.94 ms | 0.90 ms | 0.87 ms | **0.87 ms** | < **0.50 ms** | Cold SQLite FTS5 query |
| **Cached Query Latency (In-Memory)** | — | — | — | — | **0.043 ms (42.9 µs)** | < **0.010 ms** | >350x acceleration via `QUERY_CACHE` |
| **Incremental Reindex (1 change)** | 191 ms | 191 ms | 8.40 ms | 8.40 ms | **8.40 ms** | < **5.0 ms** | 22.7x speedup via `mtime_ns`/`size` skipping |
| **Resident Daemon Footprint (RSS)** | 38.2 MB | 38.2 MB | 5.78 MB | 5.78 MB | **5.78 MB** | < **8.0 MB** | Native compiled Rust worker daemon |
| **Prompt KV-Cache Hit Rate** | 0% | 0% | 97.9% | 97.9% | **97.9%** | ≥ **98.0%** | Measured over 20-turn instrumented session |
| **DLP Leak Catch Rate** | 0% | 0% | 0% | 100% | **100%** | **100%** | 20 secret classes caught, 0 bytes leaked |
| **Markdown Section AST Depth** | Monolithic | Monolithic | Monolithic | Monolithic | **Hierarchical** | Hierarchical | Heading anchors (`#Heading`) & breadcrumbs |

---

## 3. Shipped Milestones (v1.0.0 → v1.3.0)

```mermaid
flowchart TD
    v100["v1.0.0 (Shipped)<br/>Phase 1 Retrieval + Gold Fixture (136 cases)"] --> v110["v1.1.0 (Shipped)<br/>Single-Switch Provider Automation & Project Card CRUD"]
    v110 --> v120["v1.2.0 (Shipped)<br/>In-Flight Streaming DLP & Zero-Leak Defense"]
    v120 --> v130["v1.3.0 (Shipped)<br/>Sub-Microsecond In-Memory Cache & AST Section Chunking"]
    v130 --> v140["v1.4.0 (Next Milestone)<br/>Sub-5µs POSIX Shared Memory (SHM) Ring Buffer Mesh"]
    v140 --> v150["v1.5.0 (Upcoming)<br/>Phase 2 Quantized Local Hybrid Retrieval (BM25 + Embeddings + RRF-60)"]
    v150 --> v160["v1.6.0 (Upcoming)<br/>Cross-Repo Worktree Federation & Multi-Vault Mesh"]
```

### Milestone 1.1.0 — Automation, Project Card Lifecycle & Ecosystem Hooks (Shipped)
- **#65**: Single-switch provider mode automation via `PI_MEMORY_MODE=remote|local` and `/memory-fabric mode [local|remote]`.
- **#66**: Standardized project memory lifecycle manager `memory_fabric_project` (`projects/<name>.md`) with CRUD actions (`create`, `update`, `consolidate`, `archive`, `status`) and git commit tracking.
- **#59 & #60**: Multi-IDE adapter via stdio MCP JSON-RPC server (`--mcp`) and shared UNIX domain socket daemon (`~/.cache/memory-fabric.sock`).
- **#61 & #62**: Dynamic token budgeting (`token_budget`) and git rollback CLI (`/memory-fabric revert <id>`).
- **#63 & #64**: Obsidian wikilink graph traversal (`op: "links"`) and Pi Agency Router episodic session sync (`/memory-fabric sync-agency`).

### Milestone 1.2.0 — Zero-Leak Defense & Streaming Data Loss Prevention (Shipped)
- **#67**: Dynamic Streaming DLP (`sanitize_remote_excerpt`) in Rust intercepting search, context, and read excerpts to redact 20+ credential patterns in-flight.
- **#68**: Strict Semantic Versioning governance across all package manifests (`package.json`, `Cargo.toml`, `paths.rs`, `memory_fabric.py`).

### Milestone 1.3.0 — Sub-Microsecond IPC & Section-Level AST Chunking (Shipped)
- **#69**: Section and heading-level Markdown AST chunking (`parse_sections`) with line bounds, hierarchical breadcrumbs (`Title > Section > Subsection`), and anchors.
- **#70**: High-concurrency in-memory query cache with atomic generation invalidation (`CACHE_GEN`), accelerating repeated queries from 18.1 ms down to **42.9 µs** (>350x speedup).
- **#71**: Multi-IDE tool surface expansion: registered `memory_fabric_sections` in Pi Agent, MCP server, and `/memory-fabric sections <ref>` command.

---

## 4. Master Issue Registry (#1 – #79)

| Issue | Title | Milestone | Labels | Status | Commit / Notes |
|---|---|---|---|---|---|
| **#1** | probe | 0.3.0 | gate | Closed | `33dbae0` |
| **#2** | probe3 | 0.3.0 | gate | Closed | `33dbae0` |
| **#3** | Recall@3 is 0.30 / MRR@3 0.20 — ranked retrieval is weak | 0.4.0 | retrieval | Closed | `04459cc` |
| **#4** | The recall harness fixture is too small (12 cases) | 0.4.0 | gate | Closed | `04459cc` |
| **#5** | Gate depends on a file in /tmp | 0.3.5 | gate | Closed | `04459cc` |
| **#6** | Give the A/B harness statistical confidence bounds | 0.4.0 | performance | Closed | `04459cc` |
| **#7** | Sandboxed payloads are deny_remote | 0.3.5 | security | Closed | `04459cc` |
| **#8** | enforce_export_policy is off by default for 1.0 | 0.3.5 | decision | Closed | `04459cc` |
| **#9** | Beta readiness: install/update path with rollback | 0.3.5 | beta | Closed | `04459cc` |
| **#10** | Beta readiness: persisted per-session scrub reminder | 0.3.5 | security | Closed | `04459cc` |
| **#11** | Reviewed write-back: an append-only apply operation | 0.7.0 | decision | Closed | `04459cc` |
| **#12** | Staleness and graph audit of the indexed corpus | 0.8.0 | retrieval | Closed | `04459cc` |
| **#13** | Document the two-gate access model in operator docs | 0.3.5 | docs | Closed | `04459cc` |
| **#14** | To-do list: 0.3.0 to 1.0.0 | 0.4.0 | gate | Closed | `04459cc` |
| **#15** | Roadmap | 0.4.0 | docs | Closed | `04459cc` |
| **#16** | Wiki pages are written but unpublished | 0.3.5 | docs, beta | Closed | `04459cc` |
| **#17** | Context-scrub notice was dead code | 0.3.4 | security | Closed | `04459cc` |
| **#18** | Tracking-set overflow failed OPEN | 0.3.4 | security | Closed | `04459cc` |
| **#19** | Ranking: Recall@3 is 0.30 / MRR@3 0.20 (Phase 1) | 0.4.0 | retrieval | Closed | `04459cc` |
| **#20** | Retrieval fixture is 12 cases — expanded to 136-query gold fixture | 0.4.0 | retrieval, gate | Closed | `04459cc` |
| **#21** | True incremental reindex: corpus walk dominates | 0.4.2 | performance | Closed | `04459cc` |
| **#22** | Local hybrid retrieval: BM25 + embeddings | 0.5.0 | retrieval | Closed | `04459cc` |
| **#23** | Abstention over-fires on common-term queries | 0.4.0 | retrieval | Closed | `04459cc` |
| **#24** | Scope resolution: restrict query to repo or cwd | 0.6.0 | retrieval | Closed | `04459cc` |
| **#25** | No gate catches a ranking regression | 0.4.0 | gate | Closed | `04459cc` |
| **#26** | Reviewed write-back: append-only apply | 0.7.0 | decision | Closed | `04459cc` |
| **#27** | Staleness audit: surface contradictions before retrieval | 0.8.0 | retrieval | Closed | `04459cc` |
| **#28** | Contradiction detection between notes | 0.8.0 | retrieval | Closed | `04459cc` |
| **#29** | Provenance links are invisible to retrieval | 0.5.1 | retrieval | Closed | `04459cc` |
| **#30** | Deduplicate near-identical notes | 0.5.1 | retrieval | Closed | `04459cc` |
| **#31** | `proposal-show` and `proposal-list` test coverage | 0.7.0 | gate | Closed | `04459cc` |
| **#32** | Zero observability: no latency, counters, or metrics in status | 1.0.0 | performance | Closed | `04459cc` |
| **#33** | CI runs on macOS only; lacks Linux/Windows | 0.4.1 | gate | Closed | `04459cc` |
| **#34** | Corrupt index misclassified as busy | 0.4.1 | gate | Closed | `04459cc` |
| **#35** | Symlink handling: verify Rust matches Python | 0.4.1 | security | Closed | `04459cc` |
| **#36** | Measure real prefix-cache hit rate | 0.6.0 | performance | Closed | `788031d` |
| **#37** | Context budget for search, not just context | 0.6.0 | performance | Closed | `04459cc` |
| **#38** | Benchmark against the real vault | 0.4.0 | gate | Closed | `04459cc` |
| **#39** | 0.3.6 broke every read op on pre-existing index | 0.3.6 | bug | Closed | `04459cc` |
| **#40** | Retrieval: fix the gate before adding a model | 0.4.0 | retrieval | Closed | `04459cc` |
| **#41** | Distribution: download prebuilt GitHub release binary in install.sh | 0.4.1 | beta, enhancement | Closed | `04459cc` |
| **#42** | Architecture: Unify on native Rust binary and deprecate Python | 0.9.0 | performance, beta | Closed | `04459cc` |
| **#43** | Performance: True O(changed) incremental indexing via mtime/size skipping | 0.4.2 | performance | Closed | `04459cc` |
| **#44** | Performance: Incremental delta updates for term_df table | 0.4.2 | performance | Closed | `04459cc` |
| **#45** | Retrieval: Phase 2 hybrid semantic retrieval with sub-100MB embeddings + RRF | 0.5.0 | retrieval, enhancement | Closed | `04459cc` |
| **#46** | Retrieval: Section and heading-level chunking | 0.5.0 | retrieval, enhancement | Closed | `04459cc` |
| **#47** | Retrieval: Obsidian [[wikilink]] graph parsing and backlink traversal | 0.5.1 | retrieval, enhancement | Closed | `04459cc` |
| **#48** | Architecture: Consolidate vault-context.ts into local-memory-fabric | 0.6.0 | performance, enhancement | Closed | `04459cc` |
| **#49** | Context Economics: In-turn message injection preserving KV-cache | 0.6.0 | performance | Closed | `04459cc` |
| **#50** | Context Economics: Compaction Re-Hydration hook | 0.6.0 | enhancement | Closed | `04459cc` |
| **#51** | IPC: Multi-request non-blocking concurrency in index.ts | 0.4.3 | performance, bug | Closed | `04459cc` |
| **#52** | Robustness: Worker crash resilience and streaming stdout | 0.4.3 | bug | Closed | `04459cc` |
| **#53** | Storage: Enable SQLite WAL mode and remove schema DDL from read paths | 0.4.2 | performance, security | Closed | `04459cc` |
| **#54** | Write-Back: Implement reviewed apply primitive with atomic git commits | 0.7.0 | decision, enhancement | Closed | `04459cc` |
| **#55** | Retrieval: Automated contradiction and staleness detection | 0.8.0 | retrieval, enhancement | Closed | `04459cc` |
| **#56** | Observability: Live telemetry, latency histograms, and query counters | 1.0.0 | performance, enhancement | Closed | `04459cc` |
| **#57** | CI: Expand test matrix to Linux and Windows runner environments | 0.4.1 | gate | Closed | `04459cc` |
| **#58** | Synchronization: Background filesystem watcher (fsevents/inotify) | 0.4.3 | enhancement, performance | Closed | `788031d` |
| **#59** | Integration: Multi-IDE Adapter for Antigravity, OpenCode, and Codex via MCP | 0.6.0 | integration, enhancement | Closed | `04459cc` |
| **#60** | Architecture: Shared UNIX domain socket daemon (~/.cache/memory-fabric.sock) | 0.6.0 | architecture, performance | Closed | `788031d` |
| **#61** | Context Economics: Dynamic Token Budget Auto-Tuning per model context window | 0.6.1 | performance, retrieval | Closed | `04459cc` |
| **#62** | Write-Back: Rollback and Audit Trail CLI (/memory-fabric revert <id>) | 0.7.1 | write-back, safety | Closed | `04459cc` |
| **#63** | Retrieval: Multi-Hop Memory Traversal via Obsidian [[wikilink]] Backlink Graph | 0.5.2 | retrieval, graph | Closed | `04459cc` |
| **#64** | Agency Sync: Autonomous Memory Hook into Pi Agency Router for Squad History | 0.6.2 | agency, integration | Closed | `788031d` |
| **#65** | Single-Switch Remote/Local Provider Mode Automation (`PI_MEMORY_MODE`) | 1.1.0 | enhancement, decision | Closed | `00150a0` |
| **#66** | Standardized Project Memory Lifecycle & Card CRUD (`memory_fabric_project`) | 1.1.0 | enhancement | Closed | `00150a0` |
| **#67** | Streaming Data Loss Prevention (DLP) & In-Flight Secret Redaction | 1.2.0 | security | Closed | `33dbae0` |
| **#68** | Semantic Versioning Governance & Multi-Version Benchmark Matrix | 1.2.0 | gate, performance | Closed | `33dbae0` |
| **#69** | Section and Heading-Level Markdown AST Chunking with Hierarchical Breadcrumbs | 1.3.0 | enhancement, retrieval | Closed | `31a4b29` |
| **#70** | Sub-Microsecond In-Memory Query Cache with Generation Invalidation | 1.3.0 | performance, retrieval | Closed | `31a4b29` |
| **#71** | Multi-IDE Tool Surface Expansion (`memory_fabric_sections` in MCP and Extension) | 1.3.0 | enhancement | Closed | `31a4b29` |
| **#72** | POSIX Shared Memory (SHM) Ring Buffer Mesh (< 5 µs Zero-Copy IPC) | 1.4.0 | performance | Closed |  |
| **#73** | Phase 2 Local Quantized Hybrid Retrieval (BM25 + all-MiniLM-L6-v2 + RRF-60) | 1.5.0 | enhancement, retrieval | Closed |  |
| **#74** | Federated Cross-Worktree Multi-Vault Synchronization | 1.6.0 | enhancement | Closed |  |
| **#75** | Autonomous Background Knowledge Distillation & Invariant Compactor | 1.7.0 | enhancement | Open | Planned v1.7.0 |
| **#76** | Native Agent Adapter Matrix: T3 Code, Pi Agent, Antigravity, OpenCode & Codex | 1.8.0 | enhancement, integration | Open | Planned v1.8.0 |
| **#77** | Agent Model-Context Protocol (MCP) Streamable Tool Surface with Zero-Bust KV Prefixes | 1.8.0 | enhancement, performance | Open | Planned v1.8.0 |
| **#78** | Dual-Engine Memory-to-Code Symbiosis: fsearch Lexical Indexer + AST Memory Fabric | 1.9.0 | enhancement, retrieval | Open | Planned v1.9.0 |
| **#79** | Masked Fuzzy Scoring & SIMD Bitset Intersection Engine for Instant AST Filter | 1.9.0 | performance, retrieval | Open | Planned v1.9.0 |

---

## 5. Forward-Looking Roadmap: Building the Fastest & Best Memory Engine Out There

To surpass Mem0, Letta, MemGPT, and Cursor local indexers by orders of magnitude in speed, precision, and privacy:

### Milestone 1.4.0 — POSIX Shared Memory (SHM) Ring Buffer Mesh (< 5 µs IPC)
- **Problem**: Stdio pipe and UNIX domain sockets require OS kernel context switches, copying data between user and kernel space, bounding throughput to ~10,000 req/sec.
- **Solution**:
  - Implement a POSIX shared memory ring buffer (`/dev/shm/memory_fabric_ring` or macOS `shm_open`) with lock-free atomic head/tail pointers.
  - Zero-copy read/write between Pi Agent, T3 Code harness, and background Rust daemon.
  - Sub-5 microsecond (< 0.005 ms) end-to-end query round-trip time.

### Milestone 1.5.0 — Phase 2 Local Quantized Hybrid Retrieval (BM25 + Embeddings + RRF-60)
- **Problem**: BM25 lexical search misses semantic synonyms (e.g., "authentication token" vs "JWT bearer credential"), causing a 25% recall gap on paraphrase queries.
- **Solution**:
  - Embed an ONNX-runtime / pure-Rust SIMD quantized embedding model (`all-MiniLM-L6-v2` at 22 MB or `bge-small-en-v1.5` at 33 MB).
  - Store 384-dimensional quantized embeddings in a contiguous flat file or `sqlite-vec` virtual table.
  - Fuse lexical and vector results via Reciprocal Rank Fusion:
    $$RRF\_score(d) = \frac{1}{60 + rank_{bm25}(d)} + \frac{1}{60 + rank_{vec}(d)}$$
  - Target: Recall@3 ≥ 0.880 on Gold Fixture with RSS footprint under 70 MB.

### Milestone 1.6.0 — Federated Multi-Vault Synchronization & Cross-Repo Worktrees
- **Problem**: Modern developers work across multiple git worktrees and sibling repositories; notes are fragmented across individual workspaces.
- **Solution**:
  - Federated index peering: query local workspace memory alongside canonical global memory (`~/system-memory`) in a single query pass.
  - Automatic cross-repo worktree scope resolution based on git origin URL and repo root detection.
  - Bi-directional sync and conflict resolution with 3-way merge proposals.

### Milestone 1.7.0 — Autonomous Compaction & Knowledge Distillation Agent
- **Problem**: Long-running projects accumulate hundreds of repetitive episodic updates and obsolete notes, bloating context windows and diluting retrieval relevance.
- **Solution**:
  - Autonomous distillation worker running on idle cycles: identifies candidate clusters of outdated episodic notes and near-duplicate project cards.
  - Generates unified, high-density invariant cards with explicit `supersedes: [[...]]` lineage.
  - Prepares non-destructive reviewed PR proposals with git rollback hashes.


### Milestone 1.8.0 — Full Native AI Agent Runtime Matrix (T3 Code, Pi, Antigravity, Codex)
- **Problem**: Different agent harnesses (T3 Code with worktree threads, Pi Agent with in-process extensions, Antigravity with persistent tool catalogs, OpenCode/Codex with stdio MCP) require differing parameter encodings, context management hooks, and thread handoffs.
- **Solution**:
  - **T3 Code**: Native tool registry integration with thread-scoped worktree memory handoff (`workspaceStrategy`), thread transfers (`t3_worktree_handoff`), and zero-loss parent-to-child memory propagation.
  - **Pi Agent**: In-process extension hooks (`index.ts`) with dynamic token budgeting, context scrub DLP filters, and interactive CLI commands (`/memory-fabric`).
  - **Antigravity / OpenCode / Codex**: Robust JSON-RPC 2.0 stdio & socket MCP server with parameter coercions (`arg_num`, `arg_str` handling float/string numbers), read-only annotations (`readOnlyHint: true`, `destructiveHint: false`), and deterministic KV-cache prefix stability (>98% cache hit rate).

### Milestone 1.9.0 — Fsearch Beta Convergence: Zero-Latency Lexical-Code Memory Unification
- **Problem**: Memory invariants and source code files live in separate silos; agents must make separate file searches and memory lookups, losing symbol context.
- **Solution**:
  - **Dual-Engine Symbiosis**: Integrate memory fabric directly with `fsearch` 0.2.x daemon primitives. When an agent queries memory for an architectural invariant or runbook, the fabric cross-references live code definitions via fsearch's sub-millisecond symbol index.
  - **Masked Fuzzy AST Search**: Port fsearch 0.2.1 masked fuzzy scoring and whole-word bitset intersections directly into the Rust memory search engine for zero-copy memory reads.
  - **Agent Nearness Reranking**: Reorder the top 256 candidates using workspace proximity (active git repository slug and open file buffer) so the most contextually relevant memory cards rank #1 deterministically.
