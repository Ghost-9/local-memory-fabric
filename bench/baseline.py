#!/usr/bin/env python3
"""One command that captures the full metric set, so any change can be compared against a
recorded baseline instead of against memory.

Writes a dated JSON snapshot to /tmp/mf-baseline-snapshots/. Read-only with respect to the
vault; writes only that snapshot and temporary state under /tmp.

Why this exists: every measurement this session was reconstructed by hand from a different
script, and two of them were wrong (a warm-path harness reported 1.22 ms for a 3.7-16 ms
reality; a loose read detector reported 78% where the instrument could see nothing). One
instrument, one shape, comparable over time.
"""
from __future__ import annotations

import json
import os
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "rust" / "target" / "release" / "memory-fabric"
STATE = Path(os.path.expanduser("~/.pi/agent/scratch/memory-fabric"))
SNAPS = Path("/tmp/mf-baseline-snapshots")
ENV = dict(os.environ, PI_MEMORY_LOCAL_PROVIDERS="lmstudio,llama.cpp,ollama")
TERMS = ["retention", "memory", "policy", "cache"]
SNAP = {}


def serve(binary: str, requests: list[dict], env: dict | None = None):
    p = subprocess.Popen([binary, "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, text=True, env=env or ENV)
    out, lat = [], []
    try:
        for r in requests:
            t = time.perf_counter()
            p.stdin.write(json.dumps(r) + "\n")
            p.stdin.flush()
            out.append(json.loads(p.stdout.readline()))
            lat.append((time.perf_counter() - t) * 1000)
    finally:
        p.stdin.close()
        p.wait(timeout=15)
    return out, lat


def cold(binary: str, limit: int, reps: int = 4) -> float:
    """Cache-busted: a unique suffix per call, so this measures work rather than a replay."""
    reqs = [{"op": "search", "query": f"{t} b{time.perf_counter_ns()}", "scope": "all",
             "limit": limit, "provider": "llama.cpp"} for _ in range(reps) for t in TERMS]
    return statistics.median(serve(binary, reqs)[1])


def main() -> int:
    SNAP["measured_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    SNAP["engine"] = "rust"

    # --- retrieval latency, cold (the honest figure) ---
    SNAP["search_p50_ms_narrow"] = round(cold(str(BIN), 1), 2)
    SNAP["search_p50_ms_broad"] = round(cold(str(BIN), 10), 2)

    # --- context packet size and parity ---
    ctx_reqs = [{"op": "context", "query": "retrieval quality ceiling", "provider": "llama.cpp"}]
    rust_ctx = serve(str(BIN), ctx_reqs)[0][0]
    SNAP["context_chars_default"] = len(rust_ctx.get("context", ""))
    SNAP["context_char_budget"] = rust_ctx.get("char_budget")
    lock = STATE / "config.json"
    SNAP["config_enforce_export"] = json.loads(lock.read_text()).get("enforce_export_policy") if lock.exists() else None

    # --- index shape ---
    db = STATE / "index.sqlite3"
    if db.exists():
        con = sqlite3.connect(db)
        docs = con.execute("SELECT COUNT(*) FROM documents").fetchone()[0]
        SNAP["documents"] = docs
        SNAP["doc_terms"] = con.execute("SELECT COUNT(*) FROM doc_terms").fetchone()[0]
        SNAP["term_df"] = con.execute("SELECT COUNT(*) FROM term_df").fetchone()[0]
        SNAP["index_mb"] = round(db.stat().st_size / 1024 / 1024, 1)
        SNAP["bytes_per_doc"] = round(db.stat().st_size / docs) if docs else None
        con.close()

    # --- reindex cost ---
    base = Path(tempfile.mkdtemp(prefix="mf-baseline-idx-"))
    try:
        (base / "vault/projects").mkdir(parents=True)
        for i in range(200):
            (base / "vault/projects" / f"n{i}.md").write_text(
                f"# N{i}\n\nshared vocabulary term {i} retention policy\n")
        env = dict(ENV, MEMORY_FABRIC_HOME=str(base / "state"), MEMORY_FABRIC_VAULT=str(base / "vault"))
        t = time.perf_counter()
        serve(str(BIN), [{"op": "index"}], env)
        SNAP["index_full_200_ms"] = round((time.perf_counter() - t) * 1000, 1)
        (base / "vault/projects" / "n0.md").write_text("# N0\n\nedited retention policy\n")
        t = time.perf_counter()
        serve(str(BIN), [{"op": "index"}], env)
        SNAP["index_one_change_ms"] = round((time.perf_counter() - t) * 1000, 1)
    finally:
        shutil.rmtree(base, ignore_errors=True)

    # --- quality, from the gate's own fixture ---
    try:
        r = subprocess.run([sys.executable, str(ROOT / "bench" / "fixture_gate.py")],
                           capture_output=True, text=True, timeout=600, cwd=str(ROOT))
        for line in r.stdout.splitlines():
            if "cases" in line and "recall" in line:
                for tok in line.replace("|", " ").split():
                    pass
                import re
                m = re.search(r"recall@3 ([\d.]+).*?mrr@3 ([\d.]+).*?no-hit ([\d.]+)", line)
                if m:
                    SNAP["recall_at_3"] = float(m.group(1))
                    SNAP["mrr_at_3"] = float(m.group(2))
                    SNAP["no_hit_precision"] = float(m.group(3))
    except Exception as e:
        SNAP["fixture_error"] = str(e)[:120]

    # --- usage / behaviour record ---
    usage = STATE / "usage.jsonl"
    if usage.exists():
        recs = [json.loads(l) for l in usage.read_text().splitlines() if l.strip()]
        SNAP["usage_records"] = len(recs)
        SNAP["usage_kinds"] = {}
        for r in recs:
            SNAP["usage_kinds"][r.get("kind", "?")] = SNAP["usage_kinds"].get(r.get("kind", "?"), 0) + 1
        SNAP["refs_resolved"] = sum(r.get("refs", 0) for r in recs if r.get("kind") == "session_refs")
        SNAP["reads_recorded"] = sum(1 for r in recs if r.get("kind") == "read")

    # --- gate ---
    g = subprocess.run([str(ROOT / "test.sh")], capture_output=True, text=True, timeout=1800, cwd=str(ROOT))
    SNAP["gate_ok"] = g.returncode == 0
    SNAP["gate_steps"] = len([l for l in g.stdout.splitlines() if l.startswith("== ") and "/" in l])

    SNAPS.mkdir(parents=True, exist_ok=True)
    label = sys.argv[1] if len(sys.argv) > 1 else "snapshot"
    out = SNAPS / f"{time.strftime('%Y%m%dT%H%M%S')}-{label}.json"
    out.write_text(json.dumps(SNAP, indent=2, sort_keys=True))
    print(json.dumps(SNAP, indent=2, sort_keys=True))
    print(f"\nwritten: {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
