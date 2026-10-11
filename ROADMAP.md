# Roadmap — 0.4 → 1.0

Every item below is a **measured gap**, not a guess. Numbers were taken on this machine against
this vault (124 notes, 114 indexed) unless stated. Items that would require an operator policy
decision are marked **OPERATOR** and are not for an agent to pick.

## Where the numbers actually are

| Measure | Today | Target | Note |
|---|---|---|---|
| Recall@3 | **0.30** | ≥ 0.60 | The single largest quality gap. #1 below. |
| MRR@3 | **0.20** | ≥ 0.40 | |
| Search latency (Rust, warm) | 12.9 ms | — | Already fast. Not the bottleneck. |
| Search latency (Python, cold) | 78.4 ms | — | Fallback path only; 6.2x slower. |
| Full reindex (1000 notes) | 231 ms | — | |
| Incremental reindex (1 changed) | **191 ms** | < 60 ms | Only **1.2x** cheaper than a full rebuild — see #4. |
| Semantic / hybrid retrieval | **none** | BM25 + local embeddings | Purely lexical today. |
| Observability | **none** | per-op latency in `status` | Zero instrumentation. |

**The honest headline:** this is the most *governed* memory layer in the comparison set and one of
the weakest at the *retrieval* job it exists to do. Recall 0.30 is not competitive. Everything below
is sequenced so that quality is fixed before anything else is added.

---

## Milestone 0.4 — Retrieval quality

*Exit condition: Recall@5 ≥ 0.80 on a ≥100-query fixture, with abstention that does not fire on
no-hit queries.*

| # | Item | Why it exists (measured) |
|---|---|---|
| 1 | **Ranking: Recall@3 is 0.30 / MRR@3 0.20** | The top problem. Ranking is BM25-only with a `relpath` tiebreak. |
| 2 | **Fixture ≥ 100 queries** | 12 cases cannot distinguish an improvement from noise. Do this *first* or #1 is unfalsifiable. |
| 3 | **Local hybrid retrieval (BM25 + embeddings)** | QMD already indexes this vault with `embeddinggemma-300M-Q8_0`, measured Recall@5 0.30 with no reranker. Purely lexical today. |
| 4 | **True incremental reindex** | 1.2x, not 10x: the full corpus walk dominates, not the writes. Needs mtime-driven skip. |
| 5 | **Abstention calibration** | `low_discrimination` currently fires on 6 of 6 common-term probes including correct ones. |
| 6 | **Scope resolution from cwd/repo** | The architecture calls for it; a query today can hit any project. |
| 7 | **Ranking regression gate** | Nothing catches a ranking regression today — #1 could be "fixed" twice. |

## Milestone 0.5 — Memory that holds up over time

*Exit condition: contradictions and stale facts are surfaced, never silently returned.*

| # | Item | Why it exists (measured) |
|---|---|---|
| 8 | **Reviewed write-back: append-only `apply`** | Unblocked — the vault is now versioned, revert proven byte-identical. Still the highest-value missing capability. |
| 9 | **Staleness audit** | A note that silently contradicts another is returned as if both were true. |
| 10 | **Contradiction detection** | `superseded_by` is *read* from hand-written frontmatter; nothing detects when two notes disagree. |
| 11 | **Provenance links in retrieval** | Zero `[[wikilink]]` awareness. `pi-harness/07-context-and-cache` already documents the graph dependency. |
| 12 | **Deduplicate near-identical notes** | No consolidation primitive exists in either engine. |

## Milestone 0.6 — Robustness and operations

*Exit condition: nothing fails silently, and a failure is diagnosable without reading source.*

| # | Item | Why it exists (measured) |
|---|---|---|
| 13 | **`proposal-show` / `proposal-list` have ZERO test coverage** | Two of eleven ops are untested. Found by audit, not by a failing test. |
| 14 | **Per-op latency + counters in `status`** | Zero instrumentation: no `elapsed`, no `latency`, no counters anywhere. |
| 15 | **CI on Linux + Windows** | `runs-on: macos-14` only, on a project whose gate claims portability. |
| 16 | **Corrupt-index recovery** | A malformed index is classified as "busy", so a real corruption retries forever. |
| 17 | **Vault-symlink hardening parity** | Python uses `lstat`; confirm Rust matches on every platform. |

## Milestone 0.7 — Context economics

*Exit condition: measured cache stability, not asserted.*

| # | Item | Why it exists (measured) |
|---|---|---|
| 18 | **Measure real prefix-cache hit rate** | Explicitly unmeasured. Simulated scrub stability (0 shifts / 60 turns) is *not* a hit rate. |
| 19 | **Context budget aware retrieval** | `token_budget` exists but `search` has no budget at all. |
| 20 | **Fixture on the REAL vault, not synthetic** | Every benchmark runs on a generated corpus; the actual vault is untested for ranking regressions. |

---

## OPERATOR decisions (not agent-callable)

| # | Decision | Why it is not a code change |
|---|---|---|
| A | `enforce_export_policy` default for 1.0 | Decided **off** (0.3.5). Turning on needs a per-note `allow_remote` convention, and which notes may leave the machine is the operator's call. |
| B | Per-note `allow_remote` marking convention | Prerequisite for A. Privacy classification, per note. |
| C | Sandbox `deny_remote` policy (#7 open) | Store-but-never-retrieve for a remote provider is surprising; the three options trade differently. |
| D | Whether remote consolidation ever ships | All 114 notes are `deny_remote`; no consolidation primitive exists; the one prior model attempt fired on 2/2 no-hit queries and peaked at 12.4 GB. |

---

## Version plan

| Version | Scope | Gate at release |
|---|---|---|
| **0.3.6** (shipped) | Credential exposure alarms | 9 steps green |
| **0.4.0** | Milestone 0.4 — retrieval quality | + Recall@5 ≥ 0.80 on ≥100 queries |
| **0.4.1** | Ops polish from 0.6 items that are cheap (#13, #14) | Gate green, ops covered |
| **0.5.0** | Milestone 0.5 — durability | + append-only `apply` idempotent + no in-place rewrite possible |
| **0.6.0** | Milestone 0.6 — robustness | + CI matrix ≥ 2 OS, `proposal-*` covered |
| **0.7.0** | Milestone 0.7 — cache economics | + measured hit rate on a real session |
| **1.0.0** | Beta: all milestones met, OPERATOR decisions recorded | + install/update/rollback exercised on a clean machine |

**Rule for 1.0:** no feature lands without a measurement, and no OPERATOR decision is made by an
agent. That rule is the project.

## Explicit non-goals

- **A model in the retrieval path.** Measured twice, rejected twice (MLX reranker: 2/2 false
  positives on no-hit queries, 12.4 GB peak). If ever revisited, local only, proposals only.
- **Remote consolidation.** See OPERATOR decision D.
- **A hosted service, sync, or any cloud component.** The vault has none and must keep none.