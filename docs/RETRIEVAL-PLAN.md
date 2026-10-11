# Retrieval plan — measured on this machine, 2026-10-12

Everything below was measured against the **live 114-note vault**, not a synthetic corpus.
Numbers are reproducible with the commands shown. Anything unmeasured says **unmeasured**.

## Step 0: the bug that mattered more than any ranking work

Auditing the live install found that **0.3.6 broke every read op on any pre-existing index** —
`search`, `context`, `status` and `alarms` all returned `no such table: secret_alarms`. Every
existing install was broken until a re-index. Fixed in `8a0f22e` with an additive migration on
the read path and a regression test that reconstructs the old on-disk shape.

This is recorded first because it outranks everything below: the retrieval was not merely weak,
it was **returning nothing**. Any ranking work done before this fix would have been measured
against an engine that errored. The lesson — *every test builds a fresh index, which is exactly
the case that works* — is now a pinned test.

## What the failure modes actually are

Measured on the real vault, 10 representative queries:

| Case | shown | total matches | Verdict |
|---|---|---|---|
| exact rare term (`Qwen3-Reranker`) | 2 | 4 | works |
| rare term (`CliffCompaction`) | 0 | 2 | **lexical miss** |
| multi-word (`edge compaction band`) | 5 | 49 | works |
| long paraphrase | 1 | 78 | works (barely) |
| natural question | 5 | 101 | works |
| concept word (`provenance`) | 0 | 39 | **over-abstains** |
| abbreviation (`KV cache`) | 0 | 67 | **over-abstains** |

**The diagnosis is not what it looks like.** Paraphrases and natural questions *already work* —
BM25's term overlap catches them. The two real failures are:

1. **Over-abstention** (`provenance`, `KV cache`): 39 and 67 documents contain the term, and the
   `low_discrimination` gate returns nothing. The information IS in the index; the gate refuses to
   rank it. This is a **calibration bug, not a retrieval gap**, and it is free to fix.
2. **Lexical misses** on rare terms where FTS5's porter tokenizer disagrees with the query
   (`CliffCompaction`: 2 matches exist, 0 shown). Also cheap — stemmer/normalisation work.

**An embedder does not fix either of these.** Semantic search helps when the query and the answer
share no terms. Measured above, that is *not* this vault's dominant failure. This is the single
most useful thing to know before spending effort on a model.

## The model question, measured

The brief was "something small, sub-100 MB". Measured on this M1 Pro / 16 GB:

| Candidate | Size | Measured here | Verdict |
|---|---|---|---|
| `embeddinggemma-300M-Q8_0` (**already on disk**) | 318 MB | 740 MB RSS, 1.49 s cold, 769-dim, works via `llama-embedding` | Usable, **over budget** |
| `qwen3-reranker-0.6b-q8_0` (**already on disk**) | 610 MB | **3.1 GB RSS** via `llama-server --pooling rank` | Far over budget |
| `bge-small-en-v1.5` int8 | ~33 MB | not installed | The sub-100MB answer |
| `all-MiniLM-L6-v2` int8 | ~22 MB | not installed | Cheapest viable |
| `bge-base-en-v1.5` int8 | ~110 MB | not installed | Just over budget |

### A correction to the vault

The vault records that `Qwen3-Reranker-0.6B-4bit` "peaked at **12.4 GB**". That was an **MLX
Python** trial. Via GGUF on `llama.cpp` the same model measures **3.1 GB RSS**. The conclusion
"do not adopt" still holds for the default path — 3.1 GB for a ~0.3 s memory tool is still wrong —
but the recorded figure was path-specific and would mislead the next decision. Corrected in the
vault.

### What a sub-100MB embedder would and would not buy

Tested directly: `embeddinggemma` separates the three failure cases cleanly (concept query →
provenance note at cos 0.495 vs 0.287 for the runner-up; paraphrase → prefix-cost at 0.582). So
embeddings *would* help ranking generally.

But per the failure-mode table, the queries that currently fail would **still** be filtered by
the over-abstention gate before any vector ever mattered. **Fix the gate first, then measure
whether embeddings move the number.**

## The plan, in the order the evidence demands

### Phase 1 — free wins, no model (do these first)

| # | Change | Why first |
|---|---|---|
| 1 | **Recalibrate `low_discrimination`** (#23) | 2 of 3 observed failures are the gate refusing ranked data. Zero bytes. |
| 2 | **Field-aware BM25** — weight title/headings above body | `provenance` appears in note titles; BM25 weights them like body text. |
| 3 | **Tokenizer/normalisation** — camelCase split, acronym expansion | `CliffCompaction` has 2 matches and returns 0. |
| 4 | **Lower the overlap floor**, report the delta | The vault records a stricter gate was tried and cut Recall@3 0.30→0.10. Retest with both numbers. |

Exit condition: **Recall@3 above 0.30 on the #20 fixture, with no new failure mode.** If this does
not move the number, the diagnosis is wrong and we stop before adding a model.

### Phase 2 — the sub-100MB embedder, as an experiment

Only if Phase 1 plateaus. `bge-small-en-v1.5` (~33 MB int8) or `all-MiniLM-L6-v2` (~22 MB).

- Fuse with BM25 by **Reciprocal Rank Fusion**, not score addition — RRF needs no score
  calibration and cannot be destabilised by the `bm25()` scale.
- **Vectors in the same SQLite file**, so the index stays disposable and the notes stay canonical.
- Must be **optional**: absent model ⇒ byte-identical results to Phase 1. That is a gate assertion,
  not a hope.

Exit condition: recall improves **and** peak RSS, index size and query latency are reported.
If RSS is not reported, it does not ship.

### Phase 3 — reranking, only with evidence

A cross-encoder is the only thing that reliably fixes "right answer, wrong rank". The cheap one
is `cross-encoder/ms-marco-MiniLM-L-6-v2` (~90 MB). Rejected at the top of this plan because:
ranking the top-20 is where the cost is, and Phase 1 may make it unnecessary.

### Never, per the recorded evidence

- **Query expansion** — measured at **14.8 s** on this vault. Unusable.
- **A 1.7B model in the read path** — QMD's expansion model is 1.2 GB and does the same job.
- **Automatic consolidation** — see `docs/COMPARISON.md`.

## The honest summary

The retrieval job is weak at 0.30, but **the dominant failure is a 6-line gate that refuses to
rank results it already has.** No model fixes that. The order above is deliberately
gate → fields → tokenizer → embeddings, and every phase has an exit condition that can stop it.

Adding a model before Phase 1 would produce a bigger number and a worse system.