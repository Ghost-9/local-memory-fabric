#!/usr/bin/env python3
"""Alternating A/B p50 for two memory-fabric engine binaries over ONE index.

Why alternating: machine drift (thermal, background load) must not be able to
favour one arm. Why medians: the latency distribution is skewed and the mean
reports the tail. Why the ok-assertion: a fast *error* response is not a fast
search, and a timing loop that does not check `ok` will happily report
microseconds for a request the engine rejected.

Read-only with respect to memory sources; writes nothing. The index is only read
to pick query terms.

    python3 bench/ab.py BASELINE_BIN CURRENT_BIN \
        [--db PATH] [--queries 40] [--rounds 5] [--env KEY=VALUE]...

Build a baseline binary from any commit with bench/baseline.sh.
"""
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import statistics
import subprocess
import sys
import time

DEFAULT_DB = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/index.sqlite3")


def connect_readonly(path: str) -> sqlite3.Connection:
    """Open the index for reading.

    A WAL-mode database cannot be opened with mode=ro when its -shm sidecar is
    absent, because SQLite would have to create it. The failure surfaces on the
    first query rather than at connect time, so the fallback has to prove the
    handle works. Only SELECTs are ever issued through it.
    """
    for target, kwargs in ((f"file:{path}?mode=ro", {"uri": True}), (path, {})):
        con = sqlite3.connect(target, **kwargs)
        try:
            con.execute("SELECT count(*) FROM sqlite_master").fetchone()
            return con
        except sqlite3.Error:
            con.close()
    raise SystemExit(f"cannot open index for reading: {path}")


def query_terms(db: str, n: int) -> list[str]:
    con = connect_readonly(db)
    try:
        rows = con.execute(
            "SELECT term FROM term_df ORDER BY df ASC, LENGTH(term) DESC LIMIT ?", (n,)
        ).fetchall()
    finally:
        con.close()
    return [r[0] for r in rows]


def arm(binary: str, terms: list[str], env: dict[str, str], timeout: int) -> tuple[list[float], int]:
    """Return per-request latencies (ms) and the count of non-ok responses."""
    proc = subprocess.Popen(
        [binary, "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True, env=env,
    )
    lat: list[float] = []
    bad = 0
    try:
        for term in terms:
            start = time.perf_counter()
            proc.stdin.write(json.dumps({"op": "search", "query": term, "scope": "all",
                                         "provider": "llama.cpp"}) + "\n")
            proc.stdin.flush()
            resp = json.loads(proc.stdout.readline())
            lat.append((time.perf_counter() - start) * 1000)
            bad += int(not resp.get("ok"))
    finally:
        proc.stdin.close()
        proc.wait(timeout=timeout)
    return lat, bad


def pct(xs: list[float], p: float) -> float:
    xs = sorted(xs)
    return xs[max(0, min(len(xs) - 1, int(round((p / 100) * len(xs) - 0.5))))]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", help="binary built from the pre-change ref")
    parser.add_argument("current", help="binary built from the working tree")
    parser.add_argument("--db", default=DEFAULT_DB)
    parser.add_argument("--queries", type=int, default=40)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--env", action="append", default=[], metavar="KEY=VALUE")
    args = parser.parse_args()

    env = dict(os.environ)
    for pair in args.env:
        key, _, value = pair.partition("=")
        env[key] = value

    terms = query_terms(args.db, args.queries)
    if not terms:
        sys.exit(f"no query terms found in {args.db}")

    # Preflight: both arms must actually answer, or the timings are meaningless.
    for label, binary in (("baseline", args.baseline), ("current", args.current)):
        _, bad = arm(binary, terms[:1], env, timeout=10)
        if bad:
            sys.exit(f"ABORT: {label} engine ({binary}) cannot answer a query")

    latencies: dict[str, list[float]] = {"baseline": [], "current": []}
    for round_index in range(args.rounds):
        for label, binary in (("baseline", args.baseline), ("current", args.current)):
            sample, bad = arm(binary, terms, env, timeout=30)
            assert bad == 0, f"{label} had {bad} failing responses"
            latencies[label] += sample
            print(f"round {round_index} {label:8s} p50 {statistics.median(sample):7.2f} ms")

    print()
    for label in ("baseline", "current"):
        xs = latencies[label]
        print(f"{label:8s} n={len(xs)}  p50 {pct(xs, 50):7.2f}  p95 {pct(xs, 95):7.2f}  p99 {pct(xs, 99):7.2f}")

    medians = {
        label: statistics.median(
            [statistics.median(latencies[label][i:i + len(terms)])
             for i in range(0, len(latencies[label]), len(terms))]
        )
        for label in ("baseline", "current")
    }
    before, after = medians["baseline"], medians["current"]
    print(f"\nmedian of round medians: baseline {before:.2f} -> current {after:.2f} ms"
          f"   delta {100 * (after - before) / before:+.1f}%")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
