"""Score retrieval against the >=100-query gold fixture.

Reads `bench/fixture_vault.py`'s corpus and cases. Reports, per engine and combined:

  recall@k / MRR@k    over the ANSWERABLE cases only
  nohit_precision     of no-hit queries, the fraction that correctly returned nothing
  answered            how many answerable queries returned anything at all

The no-hit number matters as much as recall. A change that raises recall by answering every query
is not an improvement, and this is the number that catches that.

Usage:  python3 bench/fixture_score.py [--json] [--engine python|rust|both]
"""
from __future__ import annotations

import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PY = REPO / "memory_fabric.py"
RUST = REPO / "rust" / "target" / "release" / "memory-fabric"
sys.path.insert(0, str(REPO / "bench"))
from fixture_vault import create as create_fixture  # noqa: E402

KS = (1, 3, 5, 10)


def run_engine(engine: str, cases, home: Path, vault: Path) -> dict:
    env = {**os.environ, "MEMORY_FABRIC_HOME": str(home), "MEMORY_FABRIC_VAULT": str(vault)}
    proc = None
    if engine == "rust":
        proc = subprocess.Popen([str(RUST), "--serve"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, text=True, env=env)

    def call(req: dict) -> dict:
        if engine == "python":
            p = subprocess.run([sys.executable, str(PY), "--json-stdin"], input=json.dumps(req),
                               capture_output=True, text=True, env=env)
            try:
                return json.loads(p.stdout)
            except Exception:
                return {"ok": False, "shown": 0, "results": []}
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        return json.loads(proc.stdout.readline())

    call({"op": "index"})  # build the index once
    per_engine = []
    for case in cases:
        t0 = time.perf_counter()
        out = call({"op": "search", "query": case["query"], "limit": 10, "scope": "all",
                    "provider": "ollama", "loopback": True})
        ms = (time.perf_counter() - t0) * 1000
        refs = [r.get("source_ref") for r in out.get("results", [])]
        per_engine.append({"query": case["query"], "expected": case["expected"],
                           "kind": case["kind"], "refs": refs, "ms": ms})
    if proc:
        proc.stdin.close()
        proc.terminate()
        proc.wait(timeout=5)
    return score(per_engine)


def is_hit(expected: list[str], refs: list[str]) -> bool:
    """Whether any returned ref satisfies any expected label.

     is a wildcard: the 91 filler notes are identical apart from their entry number, so
    pinning one of them as "the" answer would be a fixture bug - any of them is equally correct.
    Labelling them with one arbitrary path made retrieval look worse than it was.
    """
    import fnmatch
    for ref in refs:
        for exp in expected:
            if "*" in exp:
                if fnmatch.fnmatch(ref, exp):
                    return True
            elif ref == exp:
                return True
    return False


def score(rows: list[dict]) -> dict:
    answerable = [r for r in rows if r["expected"]]
    nohit = [r for r in rows if not r["expected"]]
    out: dict = {"cases": len(rows), "answerable": len(answerable), "nohit": len(nohit)}
    for k in KS:
        hits = 0
        rr = []
        for r in answerable:
            top = r["refs"][:k]
            rank = next((i + 1 for i, ref in enumerate(top)
                         if is_hit(r["expected"], [ref])), 0)
            if rank:
                hits += 1
                rr.append(1.0 / rank)
        out[f"recall_at_{k}"] = round(hits / len(answerable), 4) if answerable else 0.0
        out[f"mrr_at_{k}"] = round(sum(rr) / len(rr), 4) if rr else 0.0
    out["answered"] = round(sum(1 for r in answerable if r["refs"]) / len(answerable), 4) if answerable else 0.0
    out["nohit_precision"] = round(sum(1 for r in nohit if not r["refs"]) / len(nohit), 4) if nohit else 0.0
    out["latency_p50_ms"] = round(statistics.median([r["ms"] for r in rows]), 2) if rows else 0.0
    return out


def main() -> int:
    as_json = "--json" in sys.argv
    which = "both"
    for arg in sys.argv:
        if arg.startswith("--engine="):
            which = arg.split("=", 1)[1]
    engines = ["python", "rust"] if which == "both" else [which]
    with tempfile.TemporaryDirectory(prefix="memory-fabric-fixture-") as temp:
        root = Path(temp) / "vault"
        root.mkdir(parents=True)
        cases = create_fixture(root)
        results = {}
        for engine in engines:
            home = Path(temp) / f"{engine}-state"
            results[engine] = run_engine(engine, cases, home, root)

    if as_json:
        print(json.dumps({"ok": True, "cases": len(cases), "results": results}, indent=2))
        return 0

    print(f"fixture: {len(cases)} cases")
    header = f"{'metric':<20}" + "".join(f"{e:>10}" for e in results)
    print(header)
    keys = ["recall_at_1", "recall_at_3", "recall_at_5", "recall_at_10",
            "mrr_at_3", "answered", "nohit_precision", "latency_p50_ms"]
    for key in keys:
        row = f"{key:<20}"
        for e in results:
            row += f"{results[e].get(key, 0):>10}"
        print(row)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())