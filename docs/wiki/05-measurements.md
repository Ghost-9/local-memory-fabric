# Measurements

Every claim in this repo should trace back to a number produced by `bench/`. Anything unmeasured
is labelled unmeasured.

## Latency (measured)

Alternating A/B, `bench/ab.py`, one index, median of round medians:

| change | corpus | baseline | after | delta |
|---|---|---|---|---|
| keyed `term_df` / `doc_terms` lookups | personal corpus, banded | 2.75 ms | 1.11 ms | **−59.6%** |
| same | synthetic, 1,130 docs, 3.7k terms | 2.89 ms | 2.80 ms | −3.1% |
| same | synthetic, 1,130 docs, ~40k terms | 7.57 ms | 1.11 ms | **−85.4%** |

The saving tracks **index vocabulary, not document count**. A hypothesis that it scaled with
document count was measured and refuted — 1,130 synthetic notes hold fewer distinct terms than a
small personal corpus, and showed almost no gain.

## Context cost (measured)

`memory_fabric_sandbox`: a ~13 KB payload reaches the model as **480 characters, 4%**. The rest is
retrievable via the ordinary search path.

## Two findings that were rejected on evidence

- **Scope as an indexed token.** Recommended on reasoning; measured and refuted. SQLite pushes the
  scope filter into the `WHERE` clause before `ORDER BY … LIMIT`, so scoping is **8.5× faster**
  than not scoping. Pre-filtering would have pessimised the common path.
- **Rust not setting `journal_mode=WAL`.** It does, in `index()`, exactly as Python does.

## Not measured, and not claimed

- Recall on the real corpus is **0.30 Recall@3 / 0.20 MRR@3** from an external fixture. This repo's
  own harness reports Recall@5 1.0 on 12 synthetic cases — too small to mean anything, which is
  what issue #4 fixes.
- The per-request config read cost +0.3%, inside the noise floor.
- Prefix-cache hit rate: not observable here.
