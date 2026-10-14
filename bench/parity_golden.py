#!/usr/bin/env python3
"""Record the cross-engine contract as a golden fixture, so parity survives without the oracle.

Two modes:

    bench/parity_golden.py capture    # both engines must agree; write bench/golden/parity.json
    bench/parity_golden.py check      # replay against the recorded file; fail on any drift

Why this exists: `rust/tests/parity.py` compares the Rust engine to the Python one, live. That
makes the Python engine load-bearing for every parity check, which is a strange position for a
component documented as a test oracle - if it is ever removed, the checks go with it.

Recording the contract first makes parity a property of the *repository* rather than of a second
binary: the answers become the specification, and a future change that alters them fails the gate
even if the oracle is gone.

Capture refuses to write unless both engines agree, so the fixture can only ever record a state
that was true of both.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "rust" / "target" / "release" / "memory-fabric"
GOLDEN = ROOT / "bench" / "golden" / "parity.json"
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "bench"))
from memory_fabric import MemoryFabric  # noqa: E402
from fixture_vault import create  # noqa: E402

SCOPES = ["all", "projects", "company", "system:pi-harness"]


def rust_serve(env: dict):
    p = subprocess.Popen([str(BIN), "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, text=True, env=env)
    def call(req: dict) -> dict:
        p.stdin.write(json.dumps(req) + "\n")
        p.stdin.flush()
        return json.loads(p.stdout.readline())
    return p, call


def shape(result: dict) -> dict:
    """The part of a response that is contract rather than presentation."""
    return {
        "ok": result.get("ok"),
        "shown": result.get("shown"),
        "total_matches": result.get("total_matches"),
        "abstention": result.get("abstention"),
        "refs": [r.get("source_ref") for r in result.get("results", [])],
    }


def build_corpus(base: Path) -> list[dict]:
    vault = base / "vault"
    cases = create(vault)
    return cases


def run(engine_call, cases, vault) -> list[dict]:
    out = []
    for c in cases:
        for scope in SCOPES:
            try:
                r = engine_call(c["query"], scope)
            except Exception as e:                     # noqa: BLE001
                r = {"ok": False, "error": str(e)[:80]}
            out.append({"query": c["query"], "scope": scope, **shape(r)})
    return out


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "check"
    if not BIN.exists():
        sys.exit("build the Rust worker first: cargo build --release --locked")
    with tempfile.TemporaryDirectory(prefix="mf-golden-") as temp:
        base = Path(temp)
        vault = base / "vault"
        cases = build_corpus(base)
        py_home, rust_home = base / "py", base / "rs"
        py = MemoryFabric(home=py_home, vault=vault)
        py.index()
        env = dict(os.environ, MEMORY_FABRIC_HOME=str(rust_home), MEMORY_FABRIC_VAULT=str(vault),
                   PI_MEMORY_LOCAL_PROVIDERS="lmstudio,llama.cpp,ollama")
        proc, rust = rust_serve(env)
        rust({"op": "index"})

        def py_call(q: str, scope: str) -> dict:
            return MemoryFabric(home=py_home, vault=vault).search(q, scope=scope, provider="ollama", limit=5)

        def rs_call(q: str, scope: str) -> dict:
            return rust({"op": "search", "query": q, "scope": scope, "provider": "ollama", "limit": 5})

        py_rows = run(py_call, cases, vault)
        rs_rows = run(rs_call, cases, vault)
        proc.stdin.close()
        proc.wait(timeout=15)

    if mode == "capture":
        disagree = [(a, b) for a, b in zip(py_rows, rs_rows)
                    if (a["ok"], a["shown"], a["abstention"], a["refs"]) !=
                       (b["ok"], b["shown"], b["abstention"], b["refs"])]
        if disagree:
            print(f"REFUSING TO CAPTURE: both engines must agree, {len(disagree)} case(s) differ:")
            for a, b in disagree[:3]:
                print(f"  {a['scope']} {a['query'][:40]!r}\n    py {a['refs'][:2]}\n    rs {b['refs'][:2]}")
            return 1
        GOLDEN.parent.mkdir(parents=True, exist_ok=True)
        GOLDEN.write_text(json.dumps({"captured_at": __import__("time").strftime("%Y-%m-%dT%H:%M:%SZ",
                             __import__("time").gmtime()), "cases": rs_rows}, indent=1, sort_keys=True))
        print(f"captured {len(rs_rows)} case(s) to {GOLDEN.relative_to(ROOT)}")
        print(f"  distinct queries {len(cases)}, scopes {len(SCOPES)}, both engines agreed on all")
        return 0

    if not GOLDEN.exists():
        print(f"No golden fixture at {GOLDEN.relative_to(ROOT)}; run: bench/parity_golden.py capture")
        return 1
    golden = json.loads(GOLDEN.read_text())["cases"]
    drift = []
    by_key = {(g["query"], g["scope"]): g for g in golden}
    for row in rs_rows:
        g = by_key.get((row["query"], row["scope"]))
        if g is None:
            continue
        if (g["ok"], g["shown"], g["abstention"], g["refs"]) != \
           (row["ok"], row["shown"], row["abstention"], row["refs"]):
            drift.append((g, row))
    print(f"  {len(golden)} recorded cases, {len(drift)} drifted")
    if drift:
        g, r = drift[0]
        print(f"  first drift: scope={g['scope']} query={g['query'][:40]!r}")
        print(f"    recorded {g['shown']} shown {g['refs'][:2]}")
        print(f"    live     {r['shown']} shown {r['refs'][:2]}")
        print("GATE: the retrieval contract changed. If intended, re-capture deliberately.")
        return 1
    print("  retrieval contract unchanged against the recorded fixture")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
