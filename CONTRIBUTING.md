# Contributing to Local Memory Fabric

Thank you for your interest in contributing to `local-memory-fabric`.

We hold ourselves to a strict verification bar: **every performance claim carries its live measurement, every policy invariant has an automated test, and the gate (`./test.sh`) must pass 100% green before any PR or commit is merged.**

---

## 1. Development Principles

1. **Local-First & Model-Free Core**: The retrieval and indexing engine (`rust/` and `memory_fabric.py`) must never make outbound network requests or depend on cloud LLM APIs to perform baseline indexing and retrieval.
2. **Fail-Closed Privacy**: Canonical memory notes must never leave the user's machine unredacted when operating in remote provider mode (`PI_MEMORY_MODE=remote`). The streaming Data Loss Prevention (DLP) layer must catch and redact all high-entropy credentials.
3. **No Hallucinated Benchmarks**: All latencies, recall figures, memory footprints, and KV-cache hit rates quoted in PRs or commit messages must be produced by automated scripts (`bench/fixture_score.py`, `bench/ab.py`, `test.sh`).
4. **Semantic Versioning (SemVer)**: We strictly follow SemVer. All public APIs, CLI commands, and MCP schemas maintain backwards compatibility within major version `1.x.x`.

---

## 2. Setting Up Your Environment

### Prerequisites
- **macOS** (14 Sonoma / 15 Sequoia) or **Linux** (Ubuntu 22.04+)
- **Rust** (stable toolchain: `rustup default stable`)
- **Node.js** (v20+ or v24+)
- **Python** (3.9+)

### Installation
```sh
git clone https://github.com/Ghost-9/local-memory-fabric.git
cd local-memory-fabric
./install.sh
npm install
```

---

## 3. The 11-Gate Test Suite

Before submitting any code changes, execute the full verification gate:

```sh
./test.sh
```

The gate runs 11 decisive checks in order:
1. `1/11` **Python policy suite** — Security contract, scope isolation, credential heuristics, export policy.
2. `2/11` **Extension policy suite** — Read/write/scrub permission matrix in `remote-policy.test.ts`.
3. `3/11` **Context scrub suite** — Fail-closed transcript scrubbing and credential withholding in `scrub.test.ts`.
4. `4/11` **TypeScript syntax check** — Syntax parsing across all `.ts` files.
5. `5/11` **Rust unit test suite** — `cargo test` covering tokenization, AST parsing, cache invalidation, and exit codes.
6. `6/11` **Python/Rust engine parity** — Validates that Rust and Python engines return byte-for-byte identical state and documents.
7. `7/11` **Retrieval determinism** — 100 paired queries ensuring identical tie-breaking and sorting.
8. `8/11` **Real-vault probe** — Live verification against local Markdown storage (`~/system-memory`).
9. `9/11` **Gold fixture evaluation** — 136-query gold fixture testing Recall@3, MRR@3, and no-hit abstention precision.
10. `10/11` **Exit-code taxonomy benchmark** — Validates CLI error code handling and taxonomy.
11. `11/11` **TypeScript type check** — Strict `tsc --noEmit` validation.

---

## 4. Conventional Commits & GitHub Milestones

Commit messages should follow [Conventional Commits](https://www.conventionalcommits.org/):
- `feat: ...` — New capability or tool addition.
- `fix: ...` — Bug fix or policy hardening.
- `perf: ...` — Empirical performance improvement (must quote measured before/after latency).
- `docs: ...` — Documentation or roadmap updates.
- `test: ...` — Test harness and benchmark fixture additions.

When closing GitHub issues, reference the issue number in the commit message:
```sh
git commit -m "feat(ipc): implement POSIX shared memory ring buffer (fixes #72)"
```

Every PR must be linked to its corresponding GitHub Milestone and Project board before merging.
