# bench

Reproducible performance measurement for the engine. The point of this directory
is that a perf claim in this repo can be **re-checked by someone else later**,
which it could not be while the harness lived in `/tmp`.

## Two scripts

| script | question it answers |
|--------|--------------------|
| `ab.py` | Did this change make retrieval faster on the *real* index? |
| `scale.py` | What does the win actually scale with? |
| `baseline.sh` | Builds a baseline binary from any git ref, so `/tmp` is not the archive. |

## Method (do not skip these)

1. **Alternate the arms** (`baseline`, `current`, `baseline`, ...) in one run so
   thermal or background drift cannot favour either arm.
2. **Report the median**, not the mean. Latency is skewed; the mean reports the tail.
3. **Assert `ok` on every response.** A fast *error* response is not a fast
   search. A timing loop without this check reports microseconds for a request
   the engine rejected, which is exactly how a meaningless `p50 0.05 ms` was once
   produced here.
4. **Check output equivalence** before claiming a win (see below). Faster because
   it does less is not faster.
5. **Name the variable** the delta scales with, and measure it at two levels.

## Running it

```bash
# 1. baseline binary from the commit before the change
BASE="$(bench/baseline.sh 9665114)"

# 2. real-index A/B
python3 bench/ab.py "$BASE" rust/target/release/memory-fabric

# 3. the vocabulary experiment (same document count, 10x the terms)
python3 bench/scale.py 1130 --baseline "$BASE" --pool 400    --rebuild
python3 bench/scale.py 1130 --baseline "$BASE" --pool 40000 --rebuild
```

Both scripts accept `--env KEY=VALUE` (via `MEMORY_FABRIC_VAULT` /
`MEMORY_FABRIC_HOME`) to point at a different index. Neither writes to any real
memory source; `scale.py` only writes under `/tmp`.

## Recorded results (2026-10-10, commit `b252c6c`)

Alternating A/B, n=1000 per arm, **40/40 identical result sets** between arms:

| corpus | term_df rows | baseline p50 | current p50 | delta |
|--------|-------------|--------------|-------------|-------|
| real index, 114 docs | 11,041 | 2.75 ms | **1.11 ms** | **-59.6%** |
| synthetic, 1,130 docs | 3,707 | 2.89 ms | 2.80 ms | -3.1% |
| synthetic, 1,130 docs | ~40,000 | 7.57 ms | **1.11 ms** | **-85.4%** (p95 8.04 -> 1.43) |

The saving tracks **index vocabulary, not document count**: the pre-change engine
read the entire `term_df` table on every query, and one `doc_terms` query per
candidate. Both are now keyed `IN (...)` lookups, so cost is flat in the size of
the term table.

The first hypothesis — that the win would grow with the number of documents — was
**refuted by these measurements**. 1,130 synthetic documents have fewer distinct
terms than 114 hand-written notes, and showed almost no gain. Keep that in mind
before extrapolating any number here to a bigger corpus.

Re-run on 2026-10-11 **through this harness** rather than the author's `/tmp` copy:
real index **-54.7%** (2.70 -> 1.22 ms, n=200), and 1,130 docs at 41,970 terms
**-85.8%** (7.07 -> 1.00 ms, p95 8.18 -> 1.25). Same direction and magnitude, so
the numbers are reproducible by someone who is not the author — which is the
entire point of this directory. Small differences between runs are run-to-run
variance, not regressions; treat anything inside a few percent as noise unless
per-round medians stop overlapping.

## Reproducing output equivalence

Identical medians are not proof. Compare what the two arms actually return:

```bash
python3 - <<'PY'
import json, os, sqlite3, subprocess
db = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/index.sqlite3")
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
terms = [r[0] for r in con.execute("SELECT term FROM term_df ORDER BY df ASC LIMIT 40")]
con.close()
def refs(binary):
    p = subprocess.Popen([binary, "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    out = []
    for t in terms:
        p.stdin.write(json.dumps({"op": "search", "query": t, "scope": "all", "provider": "llama.cpp"}) + "\n")
        p.stdin.flush()
        out.append([r["source_ref"] for r in json.loads(p.stdout.readline()).get("results") or []])
    p.stdin.close()
    return out
base, cur = refs("PATH/TO/BASELINE"), refs("rust/target/release/memory-fabric")
print("identical:", base == cur, "of", len(terms))
PY
```
