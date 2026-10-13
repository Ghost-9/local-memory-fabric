# Comparison — local-memory-fabric vs ten popular memory / token-saving setups

**Read this before the table.** Almost every number in the "reported" columns is a **vendor or
ranking-site claim**, not an independent measurement on this machine. The vendor behind Cognee
publishes several of the "best open-source memory" roundups that place Cognee first, without
independent benchmark numbers — so treat roundup-derived rankings as marketing.

Where I have measured something here, it says **measured** and carries the method. Where I have
not, it says **unmeasured**, and that word is load-bearing.

## The one-line version

Most of these systems are **built to send your memory somewhere**. This one is built so it
cannot. That is a real difference in threat model, not a quality score — and it costs retrieval
quality that is currently **not competitive**. Both halves of that sentence are true.

## The ten

| # | System | Stars* | Licence | Backend | Model in the read path? | Vault can stay on-disk? | Self-host without a cloud tier? |
|---|---|---|---|---|---|---|---|
| 1 | **local-memory-fabric (this)** | n/a (new) | Attribution + Permission Required | SQLite FTS5 + Rust | **No — never** | **Yes, by construction** | **Yes, zero deps** |
| 2 | Mem0 | ~63K | Apache-2.0 | Vector (+ optional graph) | **Yes** — LLM extraction | No (store is the product) | Core yes; hosted is the paved road |
| 3 | Cognee | ~30K | Apache-2.0 | Graph pipeline (SQLite/LanceDB/Kuzu) | **Yes** — cognify/memify | Partly | Yes, embedded defaults |
| 4 | Graphiti / Zep | ~29.8K | Apache-2.0 | Temporal KG (Neo4j/FalkorDB) | **Yes** — entity extraction | No | Graphiti yes; **Zep CE deprecated** |
| 5 | Letta (ex-MemGPT) | ~24.2K | Apache-2.0 | Agent runtime + memory | Agent-rewritten blocks | No | Yes |
| 6 | Hindsight | ~19.6K | MIT | Embedded PostgreSQL | **Yes** — retain/recall/reflect | No | Yes, one Docker command |
| 7 | Memvid | ~16K | Apache-2.0 | Rust; vector | Yes | No | Yes |
| 8 | Memori | ~15.6K | Apache-2.0 | — | Yes | No | Yes |
| 9 | Supermemory | — | — | TypeScript engine | Yes | No | Hosted-first |
| 10 | claude-mem | — | — | SQLite | Yes | No | Yes |

\* star counts from public listings, August 2026. They measure community, not quality.

## Measured on this machine, same task, same corpus

| Measure | This system | Notes |
|---|---|---|
| Search latency, Rust warm | **12.9 ms** | median of 15, persistent worker |
| Search latency, Python cold | 78.4 ms | 6.2x slower; fallback path only |
| Full reindex, 1000 notes | 231 ms | |
| Incremental reindex, 1 changed | **191 ms (1.2x)** | *worse than expected* — see issue #4 |
| Peak memory | **~5 MB RSS** (Rust worker) | no model, no embeddings |
| Idle processes | **0** | measured, not claimed |
| Network calls in retrieval | **0** | by construction |
| Recall@3 / MRR@3 | **0.30 / 0.20** | **the weak number** |
| Prefix-cache stability | 0 shifts / 60 simulated turns | *simulated, not a live hit rate* |

## The four axes that actually differ

| Axis | This system | The field |
|---|---|---|
| **Where memory lives** | Plain Markdown you already own and version. The index is disposable — delete it, rebuild from the notes. | Usually a database *is* the memory. Lose it, lose the corpus. |
| **Does reading send anything?** | **Never.** No network path exists in the retrieval code. | Mem0/Cognee/Hindsight call an LLM to extract and consolidate. That is the feature, and also the egress. |
| **Failure mode** | Fails **closed** (withholds). Measured: overflow withholds rather than admits. | Fails open or degrades silently. |
| **Determinism** | Byte-identical responses for identical requests, pinned in the gate. | Extraction is non-deterministic by construction. |

## Claimed token savings — and why I will not print a number

Several of these report **86–95% token reduction** and p95-latency claims of ~91% lower.
**Unmeasured here.** Two reasons I refuse to reproduce them as fact:

1. They compare against *stuffing full conversation history into context* — not against a
   retrieval layer. It is not the comparison that matters.
2. This machine already has **~97.7% cache hit rate** in long sessions, and the vault note
   `pi-harness/07-context-and-cache` establishes the real economics: **output is 59% of cost in
   long sessions, fresh input only 22%**, and resident context is the *multiplier* on every
   invalidation. A token-saving figure that ignores output volume and prefix invalidation is
   measuring the wrong thing.

The honest claim available today: this system adds **~417 tokens** of tool schema (deferred, so
usually zero) and returns **bounded** excerpts. That is not a percentage, and it is not a
benchmark.

## Where this system is genuinely worse

Stated plainly, because a comparison that only lists wins is marketing:

- **Retrieval quality.** 0.30 Recall@3 against published figures near 0.90 for systems with a
  reranker. That gap is large and it is the reason issue #1 is the top of the roadmap.
- **No semantic search.** Purely lexical. A paraphrased question that shares no rare term with
  the answer will not be found.
- **No consolidation.** None of these systems can deduplicate or merge your notes either, but
  Mem0/Hindsight/Cognee at least attempt automatic extraction.
- **One user, one machine.** No shared memory across agents or projects beyond the file tree.
- **Young.** Minutes old, versus years of production hardening.

## The honest conclusion

**This is not "better than" the ten.** On the axis that decides whether an agent is useful —
does it find the right thing — it is currently the weakest in this table, and two of the better
projects would beat it today.

What it has that none of them do is a **guarantee**: your memory cannot leave the machine during
retrieval, the notes remain the source of truth in a format you can read without any software,
and every gate that claims to protect that is one that can actually fail.

If you want the retrieval quality too, that is what 0.4.0 is for. Until then: strongest
*governance*, weakest *recall*. Both measured.