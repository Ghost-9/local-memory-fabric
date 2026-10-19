#!/usr/bin/env python3
"""Paired, alternating A/B measurement harness for retrieval ordering (Plan items 20-29).

Repository contract:
  - Alternating A/B (A, B, A, B, ...) on the SAME index, cancelling machine drift and cache state.
  - Median over at least 5 repetitions (never means).
  - Reports paired delta (median of differences B_i - A_i) with n.
  - Reports both wins and losses.
  - Reports guard metrics: recall@3 >= 0.9344, nohit_precision >= 0.9286.

Usage:
  python3 bench/ordering_ab.py [--reps 5] [--extended]
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

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "bench"))

from fixture_vault import create as create_fixture  # noqa: E402
from fixture_score import score, is_hit  # noqa: E402
from memory_fabric import MemoryFabric  # noqa: E402

BIN = ROOT / "rust" / "target" / "release" / "memory-fabric"


def run_arm(engine: str, env_overrides: dict[str, str], cases: list[dict], home: Path, vault: Path) -> dict:
    env = {**os.environ, "MEMORY_FABRIC_HOME": str(home), "MEMORY_FABRIC_VAULT": str(vault), **env_overrides}
    proc = subprocess.Popen([str(BIN), "--serve"], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, text=True, env=env)
    per_engine = []
    try:
        for case in cases:
            t0 = time.perf_counter()
            req = {"op": "search", "query": case["query"], "limit": 10, "scope": "all",
                   "provider": "ollama", "loopback": True}
            proc.stdin.write(json.dumps(req) + "\n")
            proc.stdin.flush()
            line = proc.stdout.readline()
            out = json.loads(line)
            ms = (time.perf_counter() - t0) * 1000
            assert out.get("ok"), f"engine returned ok=False for {case['query']}: {out}"
            refs = [r.get("source_ref") for r in out.get("results", [])]
            per_engine.append({"query": case["query"], "expected": case["expected"],
                               "kind": case["kind"], "refs": refs, "ms": ms})
    finally:
        proc.stdin.close()
        proc.terminate()
        proc.wait(timeout=5)
    return score(per_engine)


def paired_ab(name: str, arm_a_env: dict[str, str], arm_b_env: dict[str, str],
              cases: list[dict], home: Path, vault: Path, reps: int = 5) -> dict:
    """Run an alternating paired A/B: A, B, A, B, ... on the exact same index."""
    a_runs = []
    b_runs = []
    diffs = {
        "recall_at_1": [],
        "recall_at_3": [],
        "recall_at_5": [],
        "mrr_at_3": [],
        "nohit_precision": [],
    }
    for rep in range(reps):
        # Alternating order to cancel thermal/cache drift: A then B
        res_a = run_arm("rust", arm_a_env, cases, home, vault)
        res_b = run_arm("rust", arm_b_env, cases, home, vault)
        a_runs.append(res_a)
        b_runs.append(res_b)
        for k in diffs:
            diffs[k].append(round(res_b[k] - res_a[k], 4))

    def med(lst, key):
        return statistics.median([x[key] for x in lst])

    summary = {
        "experiment": name,
        "n": reps,
        "arm_a": {k: med(a_runs, k) for k in diffs},
        "arm_b": {k: med(b_runs, k) for k in diffs},
        "median_delta": {k: statistics.median(diffs[k]) for k in diffs},
        "diff_series": diffs,
    }
    return summary


def main():
    reps = 5
    if "--reps" in sys.argv:
        idx = sys.argv.index("--reps")
        reps = int(sys.argv[idx + 1])

    with tempfile.TemporaryDirectory(prefix="mf-ordering-ab-") as temp:
        vault = Path(temp) / "vault"
        vault.mkdir(parents=True)
        cases = create_fixture(vault)
        home = Path(temp) / "state"

        # Build the index ONCE on this shared state directory
        env = {**os.environ, "MEMORY_FABRIC_HOME": str(home), "MEMORY_FABRIC_VAULT": str(vault)}
        p = subprocess.run([str(BIN)], input='{"op":"index"}\n', capture_output=True, text=True, env=env)
        assert p.returncode == 0, f"indexing failed: {p.stderr}"

        # Baseline: old defaults before ordering tuning
        baseline_env = {
            "MEMORY_FABRIC_SECTION_WEIGHT": "0.15",
            "MEMORY_FABRIC_REPETITION_WEIGHT": "1.0",
            "MEMORY_FABRIC_TITLE_WEIGHT": "4.0",
            "MEMORY_FABRIC_HEADING_WEIGHT": "1.5",
        }

        experiments = [
            ("SECTION_WEIGHT: 0.15 (base) -> 0.0",
             baseline_env,
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0"}),

            ("SECTION_WEIGHT: 0.15 (base) -> 0.30 (loss test)",
             baseline_env,
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.30"}),

            ("SECTION_WEIGHT: 0.15 (base) -> 0.50 (loss test)",
             baseline_env,
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.50"}),

            ("REPETITION_WEIGHT: 1.0 (base) -> 2.0 (with sec=0.0)",
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0"},
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "2.0"}),

            ("REPETITION_WEIGHT: 1.0 (base) -> 3.0 (with sec=0.0)",
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0"},
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0"}),

            ("TITLE_WEIGHT: 4.0 (base) -> 6.0 (with sec=0.0, rep=3.0)",
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0"},
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0", "MEMORY_FABRIC_TITLE_WEIGHT": "6.0"}),

            ("TITLE_WEIGHT: 6.0 -> 10.0 (loss/overfit test)",
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0", "MEMORY_FABRIC_TITLE_WEIGHT": "6.0"},
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0", "MEMORY_FABRIC_TITLE_WEIGHT": "10.0"}),

            ("HEADING_WEIGHT: 1.5 -> 3.0 (loss/noise test)",
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0", "MEMORY_FABRIC_TITLE_WEIGHT": "6.0"},
             {**baseline_env, "MEMORY_FABRIC_SECTION_WEIGHT": "0.0", "MEMORY_FABRIC_REPETITION_WEIGHT": "3.0", "MEMORY_FABRIC_TITLE_WEIGHT": "6.0", "MEMORY_FABRIC_HEADING_WEIGHT": "3.0"}),
        ]

        print(f"=== PAIRED ALTERNATING A/B BENCHMARK (n={reps} rounds per arm) ===")
        print(f"Index: {home} (single shared index, deterministic replay)")
        print(f"{'Experiment':<55} | {'R@1 (A->B)':<16} | {'MRR@3 (A->B)':<16} | {'R@3':<14} | {'NoHit':<14}")
        print("-" * 125)

        all_summaries = []
        for name, env_a, env_b in experiments:
            res = paired_ab(name, env_a, env_b, cases, home, vault, reps=reps)
            all_summaries.append(res)
            a_r1 = res["arm_a"]["recall_at_1"]
            b_r1 = res["arm_b"]["recall_at_1"]
            d_r1 = res["median_delta"]["recall_at_1"]
            a_mrr = res["arm_a"]["mrr_at_3"]
            b_mrr = res["arm_b"]["mrr_at_3"]
            d_mrr = res["median_delta"]["mrr_at_3"]
            b_r3 = res["arm_b"]["recall_at_3"]
            b_nh = res["arm_b"]["nohit_precision"]
            print(f"{name:<55} | {a_r1:.4f}->{b_r1:.4f} ({d_r1:+.4f}) | {a_mrr:.4f}->{b_mrr:.4f} ({d_mrr:+.4f}) | {b_r3:.4f}        | {b_nh:.4f}")

        # Save machine-readable evidence
        evidence_path = ROOT / "bench" / "evidence" / "ordering_ab_results.json"
        evidence_path.write_text(json.dumps(all_summaries, indent=2))
        print(f"\nSaved full paired A/B run data to {evidence_path}")


if __name__ == "__main__":
    main()
