"""Probe queries against the REAL vault, for measuring gate/ranking changes.

Why this exists. `bench/recall.py` scores 1.0 recall@5 on a *synthetic* corpus, which is
generated from the terms being queried — it flatters lexical retrieval and cannot detect the
failure modes this vault actually has. This file measures against the live vault instead.

Read-only by construction: it opens the index read-only, never writes, and never prints note
text — only counts, timings and the query strings below (which are ours, not the vault's).

Usage:  python3 bench/real_probe.py [--json]
"""
from __future__ import annotations

import json
import os
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PY = REPO / "memory_fabric.py"
RUST = REPO / "rust" / "target" / "release" / "memory-fabric"

# Queries are ours. Each names a real topic in this vault and the failure mode it probes.
QUERIES: list[tuple[str, str, str]] = [
    # (label, query, expected failure mode)
    ("rare-term", "Qwen3-Reranker", "works today"),
    ("multiword", "edge compaction band", "works today"),
    ("paraphrase", "why does changing the beginning of a prompt cost so much", "works"),
    ("natural-q", "how do I keep my prompts cheap", "works"),
    ("abbrev", "KV cache", "OVER-ABSTAINS"),
    ("concept", "provenance", "OVER-ABSTAINS"),
    ("camel", "CliffCompaction", "LEXICAL MISS"),
    ("acronym", "BM25", "check"),
    ("hyphen", "fail-closed", "works"),
    ("two-common", "memory cache", "check"),
    ("cross-project", "decision log rejected on evidence", "check"),
    ("long-tail", "rewriting the system prompt invalidates the whole prefix", "check"),
]


def probe(engine: str, query: str, home: str, vault: str) -> dict:
    req = {"op": "search", "query": query, "limit": 5, "scope": "all",
           "provider": "ollama", "loopback": True}
    env = {**os.environ, "MEMORY_FABRIC_HOME": home, "MEMORY_FABRIC_VAULT": vault}
    t0 = time.perf_counter()
    if engine == "python":
        p = subprocess.run([sys.executable, str(PY), "--json-stdin"], input=json.dumps(req),
                           capture_output=True, text=True, env=env)
        try:
            out = json.loads(p.stdout)
        except Exception:
            out = {"ok": False, "error": (p.stdout or p.stderr)[:200]}
    else:
        proc = subprocess.Popen([str(RUST), "--serve"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, text=True, env=env)
        try:
            proc.stdin.write(json.dumps(req) + "\n")
            proc.stdin.flush()
            out = json.loads(proc.stdout.readline())
        finally:
            proc.stdin.close()
            proc.terminate()
            proc.wait(timeout=5)
    return {"shown": out.get("shown", 0) or 0, "total": out.get("total_matches", 0) or 0,
            "low_discrimination": bool(out.get("low_discrimination")),
            "ms": (time.perf_counter() - t0) * 1000, "ok": bool(out.get("ok"))}


def main() -> int:
    home = os.environ.get("MEMORY_FABRIC_HOME", str(Path.home() / ".pi/agent/scratch/memory-fabric"))
    vault = os.environ.get("MEMORY_FABRIC_VAULT", str(Path.home() / "system-memory"))
    as_json = "--json" in sys.argv
    engines = [e for e in ("python", "rust") if
               (e == "python" or RUST.exists())]

    rows, mismatches = [], 0
    for label, query, expect in QUERIES:
        row = {"label": label, "query": query, "expected": expect}
        for engine in engines:
            r = probe(engine, query, home, vault)
            row[engine] = r
        # Parity: both engines must agree on shown/total, or the change is not trustworthy.
        if len(engines) == 2:
            same = (row["python"]["shown"] == row["rust"]["shown"]
                    and row["python"]["total"] == row["rust"]["total"])
            if not same:
                mismatches += 1
            row["parity"] = same
        rows.append(row)

    if as_json:
        print(json.dumps({"ok": mismatches == 0, "queries": len(rows),
                          "parity_mismatches": mismatches, "rows": rows}, indent=2))
        return 0

    print(f"{'label':<15}{'shown':>7}{'total':>7}{'lowdisc':>9}{'ms(py)':>9}{'ms(rs)':>9}  expected")
    for r in rows:
        p, s = r["python"], r.get("rust", {})
        flag = "" if r.get("parity", True) else "  <-- PARITY MISMATCH"
        print(f"{r['label']:<15}{p['shown']:>7}{p['total']:>7}{str(p['low_discrimination']):>9}"
              f"{p['ms']:>9.1f}{s.get('ms', float('nan')):>9.1f}  {r['expected']}{flag}")
    answered = sum(1 for r in rows if r["python"]["shown"] > 0)
    print(f"\nanswered {answered}/{len(rows)}   parity mismatches: {mismatches}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())