#!/usr/bin/env python3
"""Assert that an identical request against an unchanged index produces a byte-identical
response, in both engines.

Honest scope of what this proves. It is a *regression* guard, not a proof of SQLite's tie
order: on the fixture below it passes with or without the explicit `ORDER BY rank, relpath`
tiebreak, because FTS5 happens to walk rows in relpath order on this corpus anyway. So it will
catch a genuine nondeterminism — a clock or map iteration leaking into a response, or the two
engines disagreeing on ordering — but it cannot demonstrate that the tiebreak is load-bearing.

The tiebreak is kept because it makes ordering a contract rather than an accident of the
current query plan: it costs nothing and survives a plan change that would otherwise move the
prefix. The prefix cache depends on the second kind. What is NOT claimed: that removing the
tiebreak would be caught by this check. It has been tried, and it does not.

Read-only with respect to the knowledge base; writes nothing outside a temp dir. Exits 1 on any
divergence, with the query and both payloads quoted so a failure is actionable in CI.
"""
from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BIN = REPO / "rust" / "target" / "release" / "memory-fabric"
SCRIPT = REPO / "memory_fabric.py"
sys.path.insert(0, str(REPO))
from memory_fabric import MemoryFabric  # noqa: E402

# Deliberately tie-heavy: identical bodies guarantee equal BM25 scores, so ordering has nothing
# but the tiebreak to rely on. A query with distinct scores would pass even with no tiebreak.
QUERIES = [
    ("identical words", "all"),
    ("shared vocabulary note", "all"),
    ("zzz mmm aaa", "all"),
    ("tie", "all"),
]
REPEATS = 25


def build(base: Path) -> Path:
    vault = base / "vault"
    (vault / "projects").mkdir(parents=True)
    for name in ("zzz.md", "mmm.md", "aaa.md", "kkk.md"):
        # Identical terms, identical length: engineered to tie.
        (vault / "projects" / name).write_text(
            "# Same heading\n\nidentical words here for the tie\n", encoding="utf-8"
        )
    (vault / "projects" / "unique.md").write_text(
        "# Only here\n\nzirconium quasars distinct\n", encoding="utf-8"
    )
    return vault


def rust_serve(env: dict) -> subprocess.Popen:
    return subprocess.Popen([str(BIN), "--serve"], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            text=True, env=env)


def main() -> int:
    if not BIN.exists():
        sys.exit("build the Rust worker first: cargo build --release --locked")
    failures = []
    with tempfile.TemporaryDirectory(prefix="mf-determinism-") as temp:
        base = Path(temp)
        vault = build(base)
        py_home, rust_home = base / "py-state", base / "rust-state"
        MemoryFabric(home=py_home, vault=vault).index()
        env = dict(os.environ, MEMORY_FABRIC_HOME=str(rust_home), MEMORY_FABRIC_VAULT=str(vault),
                   PI_MEMORY_LOCAL_PROVIDERS="lmstudio,llama.cpp,ollama")
        proc = rust_serve(env)

        def rust(request: dict) -> str:
            proc.stdin.write(json.dumps(request) + "\n")
            proc.stdin.flush()
            return proc.stdout.readline().strip()

        rust({"op": "index"})
        problems = []
        for query, scope in QUERIES:
            seen: dict[str, str] = {}
            for _ in range(REPEATS):
                py = MemoryFabric(home=py_home, vault=vault).search(
                    query, scope=scope, provider="llama.cpp")
                rs = json.loads(rust({"op": "search", "query": query, "scope": scope,
                                      "provider": "llama.cpp"}))
                for label, payload in (("python", py), ("rust", rs)):
                    blob = json.dumps(payload, sort_keys=True)
                    if label in seen and seen[label] != blob:
                        problems.append((query, label, seen[label], blob))
                    seen[label] = blob
            # Cross-engine: the two engines must return the same ordering too, or a
            # provider switch inside one session moves the prefix.
            py_blob = json.dumps(
                [r["source_ref"] for r in MemoryFabric(home=py_home, vault=vault).search(
                    query, scope=scope, provider="llama.cpp").get("results", [])])
            rs_blob = json.dumps(
                [r["source_ref"] for r in json.loads(
                    rust({"op": "search", "query": query, "scope": scope,
                          "provider": "llama.cpp"})).get("results", [])])
            if py_blob != rs_blob:
                problems.append((query, "cross-engine", py_blob, rs_blob))
        proc.stdin.close()
        proc.wait(timeout=15)

        checked = len(QUERIES) * REPEATS
        if problems:
            failures.extend(problems)

    if failures:
        print(json.dumps({"ok": False, "determinism_problems": len(failures)}))
        for query, label, first, second in failures[:3]:
            print(f"\n  query={query!r} engine={label}")
            print(f"    first : {first[:200]}")
            print(f"    again : {second[:200]}")
        print("\nGATE: retrieval is not deterministic — the prefix cache will break.")
        return 1
    print(f"  {checked} paired requests across {len(QUERIES)} engineered-tie queries:")
    print("  byte-identical within each engine, and identical ordering across engines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
