#!/usr/bin/env python3
"""Scale A/B on a synthetic corpus: what does the win actually scale with?

The key lesson behind this script: the first hypothesis was wrong. The saving
does NOT scale with document count, it scales with **index vocabulary** (rows in
term_df), because the pre-change engine read that whole table on every query.
A corpus of 1,130 documents built from a small word pool has FEWER distinct
terms than 114 hand-written notes, and showed almost no difference.

So this script lets you hold document count fixed and vary the vocabulary:

    bench/scale.py 1130 --rebuild                        # ~3.7k terms
    MF_SCALE_POOL=40000 bench/scale.py 1130 --rebuild    # ~40k terms

It refuses to report timings if any response is not ok. Writes only under /tmp;
never touches a real memory source.
"""
from __future__ import annotations

import argparse
import json
import os
import random
import shutil
import sqlite3
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CURRENT_DEFAULT = str(REPO / "rust" / "target" / "release" / "memory-fabric")

TOPIC = ["retention", "policy", "cache", "harness", "vector", "ledger", "scope",
         "engine", "profile", "budget", "token", "index", "probe", "gate", "audit"]


def pct(xs: list[float], p: float) -> float:
    xs = sorted(xs)
    return xs[max(0, min(len(xs) - 1, int(round((p / 100) * len(xs) - 0.5))))]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("notes", nargs="?", type=int, default=1130)
    parser.add_argument("--baseline", required=True, help="binary built from the pre-change ref")
    parser.add_argument("--current", default=CURRENT_DEFAULT)
    parser.add_argument("--pool", type=int, default=int(os.environ.get("MF_SCALE_POOL", "400")),
                        help="size of the shared vocabulary pool")
    parser.add_argument("--queries", type=int, default=200)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--rebuild", action="store_true", help="regenerate the corpus and index")
    args = parser.parse_args()

    base = Path(f"/tmp/mf-scale-{args.notes}-p{args.pool}")
    vault, home = base / "vault", base / "state"
    env = dict(os.environ, MEMORY_FABRIC_VAULT=str(vault), MEMORY_FABRIC_HOME=str(home))

    if args.rebuild:
        shutil.rmtree(base, ignore_errors=True)

    random.seed(7)
    pool = [f"shared{i}" for i in range(args.pool)]
    draws = 90 if args.pool <= 1000 else 120
    (vault / "projects").mkdir(parents=True, exist_ok=True)
    for i in range(args.notes):
        words = [f"uniq{i:05d}"] + random.choices(pool, k=draws) + random.choices(TOPIC, k=20) + [f"num{i}"]
        (vault / "projects" / f"n{i:05d}.md").write_text(
            f"# Note {i}\ntags: {random.choice(TOPIC)}\n\n{' '.join(words)}\n", encoding="utf-8")

    def serve(binary: str, requests: list[dict], time_them: bool) -> tuple[list[float], int]:
        proc = subprocess.Popen([binary, "--serve"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)
        lat: list[float] = []
        bad = 0
        try:
            for request in requests:
                start = time.perf_counter()
                proc.stdin.write(json.dumps(request) + "\n")
                proc.stdin.flush()
                resp = json.loads(proc.stdout.readline())
                if time_them:
                    lat.append((time.perf_counter() - start) * 1000)
                bad += int(not resp.get("ok"))
        finally:
            proc.stdin.close()
            proc.wait(timeout=30)
        return lat, bad

    if not list(home.rglob("*.sqlite3")):
        start = time.perf_counter()
        _, bad = serve(args.current, [{"op": "index"}], False)
        assert bad == 0, "indexing failed"
        print(f"indexed {args.notes} notes in {time.perf_counter() - start:.2f} s")

    db = next(str(p) for p in home.rglob("*.sqlite3"))
    # A WAL-mode index without its -shm sidecar cannot be opened read-only; the
    # failure appears on the first query, so the fallback must prove the handle.
    for target, kwargs in ((f"file:{db}?mode=ro", {"uri": True}), (db, {})):
        con = sqlite3.connect(target, **kwargs)
        try:
            con.execute("SELECT count(*) FROM sqlite_master").fetchone()
            break
        except sqlite3.Error:
            con.close()
    else:
        sys.exit(f"cannot open index for reading: {db}")
    documents = con.execute("SELECT COUNT(*) FROM documents").fetchone()[0]
    terms_total = con.execute("SELECT COUNT(*) FROM term_df").fetchone()[0]
    terms = [r[0] for r in con.execute(
        "SELECT term FROM term_df WHERE df BETWEEN 2 AND ? ORDER BY df DESC, LENGTH(term) DESC LIMIT ?",
        (max(3, documents // 4), args.queries))]
    con.close()
    print(f"index: {documents} docs, {terms_total} terms, {len(terms)} probe queries (pool {args.pool})\n")

    requests = [{"op": "search", "query": t, "scope": "all", "provider": "llama.cpp"} for t in terms]
    for label, binary in (("baseline", args.baseline), ("current", args.current)):
        _, bad = serve(binary, requests[:1], False)
        if bad:
            sys.exit(f"ABORT: {label} engine cannot answer; timings would be meaningless")

    lat: dict[str, list[float]] = {"baseline": [], "current": []}
    for round_index in range(args.rounds):
        for label, binary in (("baseline", args.baseline), ("current", args.current)):
            sample, bad = serve(binary, requests, True)
            assert bad == 0, f"{label} had {bad} failing responses"
            lat[label] += sample
            print(f"round {round_index} {label:8s} p50 {statistics.median(sample):7.2f} ms")

    print()
    for label in ("baseline", "current"):
        xs = lat[label]
        print(f"{label:8s} n={len(xs)}  p50 {pct(xs, 50):7.2f}  p95 {pct(xs, 95):7.2f}  p99 {pct(xs, 99):7.2f}")

    before, after = pct(lat["baseline"], 50), pct(lat["current"], 50)
    print(f"\nmedian p50: {before:.2f} -> {after:.2f} ms   delta {100 * (after - before) / before:+.1f}%")
    print("remember: the variable that matters is term_df cardinality, not document count")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
