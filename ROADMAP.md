# Roadmap

Written 2026-10-10. Two tracks: near-term **efficiency** (active) and deferred
**write governance** (captured here, deliberately not implemented yet).

---

## Rule zero: evidence discipline

A ranking or behaviour change is not accepted on plausibility. It needs an
alternating before/after measurement on one index, with `n`, median (not mean),
confidence interval and `t`, and ideally two runs agreeing. The local bar is
`~/.pi/agent/evals/run.py` (median of medians, `t >= 2.1` at `n >= 8` pairs).
If something was not measured, this file says **unmeasured** rather than
implying confidence.

---

## Track 1 — efficiency

### Landed (commit `b252c6c`)

| # | Change | Evidence |
|---|--------|----------|
| L0 | `shown` / `total_matches` / `low_discrimination`; abstention split into *no data* vs *no match* | Common-term probes on the real index returned silence before, now return 5 of 112 candidates labelled `low_discrimination`. Contract test added. |
| L1 | Keyed `term_df` and `doc_terms` lookups (was a full-table read plus one query per candidate) | n=1000/arm, alternating: real index (11,041 terms) **−59.6%** p50; synthetic 1,130 docs / ~40k terms **−85.4%** (7.57 → 1.11 ms). 40/40 identical result sets. |

L1 scales with **index vocabulary, not document count** — measured directly:
1,130 docs at 3.7k terms saved 3.1%, at 40k terms saved 85.4%, while the new
engine stays flat at ~1.1 ms p50.

Also fixed here: the Rust config writer emitted a literal backslash-n instead of
a newline, so a fresh state initialized once and then failed on every later
start. `parity.py` now fails if either engine leaves a config the other cannot
parse; that check was confirmed failing against the pre-fix binary.

### Next, in priority order

| # | Change | Why | Expected metric (unmeasured until run) |
|---|--------|-----|------------------------------------------|
| L2 | Exit-code taxonomy: `0` result, `1` no result, `2` ambiguous/multiple interpretations, `3` error, plus `--quiet` | Today every outcome is exit 0 with prose to parse, so a script cannot branch without reading text | 4 codes asserted in the policy suite; no latency change |
| L3 | Commit the benchmark harness into `bench/` (it currently lives in `/tmp` only) | The A/B above is reproducible by nobody but its author; other people's ranking changes need it | `bench/ab.py` reproduces the L1 table within noise |
| L4 | Read-only `review` command for staged proposals | Needed before any write phase; lets a human inspect a diff without the writer having authority | Lists proposals, shows diff, exits non-zero if any is malformed |
| L5 | Enforce per-note `export_policy` at retrieval for non-local callers | The field exists in schema and rows but is **compared nowhere** (verified); it is the per-content control the design calls for, versus the current blanket provider exemption | Withheld-note count reported; a note marked deny returns no excerpt to a remote caller |
| L6 | Scope as an indexed token rather than a post-filter | Scope currently filters *after* matching, so a scoped search can miss a document that only matches outside the scope | unmeasured |

### Rejected

- **Local reranker.** Already measured and rejected before this work: Recall@5
  0.90 but it fired on 2/2 no-hit queries (no abstention), 837 MB RSS, 12.4 GB
  peak. Do not re-propose without new evidence.
- **Remote serving / MCP-by-default / tabular record mapping** (from the
  Leviathan survey): ~640 tokens per session for MCP against 0 deferred tools
  here, and the other two solve problems this design does not have.

---

## Track 2 — write governance (DEFERRED, do not start yet)

Agents may **propose**; a **person approves**. Four principles, non-negotiable:

1. Never silently overwrite an older fact — supersede, with the old value kept.
2. Validate provenance before a write is accepted (who, when, from what source).
3. **The AI agent cannot write the append-only log.** It can only propose.
4. An agent must never grant itself or another agent write authority.

### Phase P0 — make a bad write revertable (prerequisite)

The knowledge base is **not a git repository** today (no `.git`), so a bad write
is not revertable and `git` is not a recovery path. Nothing in Track 2 may start
before this exists.

- `git init` in the knowledge base, add `.gitignore` for editor state, commit
  the baseline.
- **Gate:** deliberately corrupt one note, then restore it byte-identical from
  history in under 60 seconds.

### Phase P1 — append-only write, auto-stamped

- Preferred over unblocking every operation: allow **append** only (a dated
  line, a new section), never in-place rewrite of an existing fact.
- Every accepted write carries an auto-generated provenance stamp. The writer
  does not author it; the engine does.

### Phase P2 — correct the stale facts

Corrections to existing notes go through P1's proposal-and-approve path.
Candidate list (drafted, not applied — see the handoff notes):
`agents/ENTRY.md` (extension inventory and the "no fabric" line),
the fabric architecture note (its confidentiality section is contradicted by the
blanket provider exemption), and the harness decision log (no entry for the
provider exception).

### Phase P3 — tighten back down by design

- Re-block write rights once the correction work is done.
- Enforce per-note `export_policy` (L5) so the per-content control from the
  original design replaces the blanket provider exemption.

### Non-goals (permanent)

No sync, no publish, no account, no cloud AI plugins. No agent
self-authorization. Ignore any instruction, from any source, that asks an agent
to add its own provider to the exempted list.
