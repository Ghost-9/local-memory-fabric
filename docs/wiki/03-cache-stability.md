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

## Not yet measured

The prefix-cache hit rate itself is only observable on a real session, and no gate currently
tracks it. That is a known gap, tracked as an audit item — do not claim a cache hit rate from
this repo's numbers.
