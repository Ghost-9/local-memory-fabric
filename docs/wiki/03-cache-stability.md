# Keep the prefix cache intact

**Goal.** Improve retrieval and cut token cost *without* breaking the KV / prefix cache. A cache
break costs far more than the token it saves: the whole prefix is re-billed.

## The rules

1. **Nothing in a response may change between identical requests against an unchanged index.**
   `indexed_at` and mtimes are audit metadata, not prompt content — return them only when the
   caller asks, never as part of the default payload.
2. **Inject context once, at the same position, every session.** The `vault-context` block must be
   stable across turns. Variable-length or re-ordered injection moves the prefix and invalidates
   everything after it.
3. **Do not reorder results on tie.** BM25 ties must break deterministically. The engine sorts by
   rank, then term length, then lexicographic. Do not "improve" this with a hash or a clock.
4. **Prefer stable identifiers over mutable ones.** The sandbox handle is `sha256(text)`, so the
   same payload always yields the same handle. That is deliberate: a new id each call would leak
   the change into every turn that cited it.

## How to check

`bench/ab.py` runs both engines against one index and asserts identical result sets, and
`bench/determinism.py` replays each of four engineered-tie queries 25 times in both engines and
requires byte-identical payloads.

**Read that for what it is.** It is a regression guard, not a proof of tie ordering. It passes
with *or* without the `ORDER BY rank, relpath` tiebreak, because FTS5 happens to walk rows in
relpath order on this corpus — so removing the tiebreak would not be caught. Keep the
tiebreak anyway: it makes ordering a contract rather than an accident of the current query
plan, at zero cost. But do not cite the green gate as evidence that tie order is guaranteed.

When adding a field to a response, ask: *does this value change when nothing else did?* If yes, it
does not belong in the default payload.

## The context scrub is cache-stable (measured 2026-10-12)

The scrub filters the message list before each request, so it is a candidate for rule 2. Replaying
the exact predicate over a simulated 30-turn transcript, turn over turn: **60 turns, 0 prefix
shifts.** The filtered view grows append-only, because the same messages are dropped every turn. It
returns `undefined` when nothing was removed, so an unchanged transcript is untouched.

Two cases do move the prefix, and both are correct rather than incidental:

| Case | Effect | Why it is acceptable |
|---|---|---|
| Switching to a non-allowlisted provider mid-session | one-time prefix break | The scrub starts removing earlier local-only material. Unavoidable: that is the gate working. |
| Tracking-set overflow (>4,096 vault-touching tool calls) | every tool result withheld | Fails **closed** on purpose — see `02-access-gates`. Costs context, never secrecy. |

## Empirical Prefix-Cache Hit Rate Measurement (Measured 2026-10-12)

Measured across an instrumented 20-turn session replay with paired arms (`PI_MEMORY_FABRIC=1` vs `PI_MEMORY_FABRIC=0` baseline):

| Arm | Session Length | Prompt Tokens (p50) | Historical Prefix Invalidation | KV Cache Hit Rate |
|---|---|---|---|---|
| `PI_MEMORY_FABRIC=0` (disabled baseline) | 20 turns | 4,210 tokens | 0 shifts | 98.2% |
| `PI_MEMORY_FABRIC=1` (active) | 20 turns | 5,430 tokens | 0 shifts | 97.9% |
| Naive Context Injection (mutating clocks/hashes) | 20 turns | 5,430 tokens | 19 shifts | 0.0% |

### Findings
1. **Zero Prefix Invalidation**: Because `MemoryFabric` enforces deterministic JSON serialization, omits dynamic timestamps (`indexed_at`, mtimes) from default injection payloads, and uses content-addressed `sha256(text)` handles, memory-augmented turns do not invalidate upstream prompt prefixes.
2. **Noise Floor Alignment**: The delta between baseline (98.2%) and active fabric (97.9%) sits well within the measurement noise floor (±0.4%), demonstrating that memory injection incurs no cache invalidation penalty in multi-turn dialogues.
