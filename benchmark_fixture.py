#!/usr/bin/env python3
"""Aggregate-only Python baseline on a reproducible synthetic fixture."""
from __future__ import annotations

import json
import resource
import statistics
import sys
import tempfile
import time
from pathlib import Path

from benchmarks.synthetic import create_synthetic_vault
from memory_fabric import MemoryFabric


def percentile(values: list[float], p: float) -> float:
    xs = sorted(values)
    return xs[min(len(xs) - 1, int((len(xs) - 1) * p + 0.999999))] if xs else 0.0


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="memory-fabric-python-bench-") as temp:
        base = Path(temp)
        vault = base / "synthetic-vault"
        cases = create_synthetic_vault(vault)
        fabric = MemoryFabric(home=base / "state", vault=vault)
        start = time.perf_counter()
        indexed = fabric.index()
        index_ms = (time.perf_counter() - start) * 1000
        latencies = []
        results = []
        for case in cases:
            start = time.perf_counter()
            value = fabric.search(case["query"], provider="ollama", limit=5)
            latencies.append((time.perf_counter() - start) * 1000)
            results.append([row["source_ref"] for row in value.get("results", [])])
        positive = [case for case in cases if case["expected"]]
        negative = [case for case in cases if not case["expected"]]
        hits = 0
        reciprocal_ranks = 0.0
        false_positives = 0
        for case, refs in zip(cases, results):
            expected = set(case["expected"])
            if expected:
                rank = next((i + 1 for i, ref in enumerate(refs) if ref in expected), None)
                if rank:
                    hits += 1
                    reciprocal_ranks += 1 / rank
            elif refs:
                false_positives += 1
        rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        if sys.platform != "darwin":
            rss *= 1024
        report = {
            "dataset": "synthetic-111-indexed-documents",
            "fixture_cases": len(cases), "positive_cases": len(positive), "negative_cases": len(negative),
            "recall_at_5": round(hits / len(positive), 4) if positive else 0.0,
            "mrr_at_5": round(reciprocal_ranks / len(positive), 4) if positive else 0.0,
            "negative_false_positives": false_positives,
            "negative_false_positive_rate": round(false_positives / len(negative), 4) if negative else 0.0,
            "search_ms_p50": round(statistics.median(latencies), 3) if latencies else 0.0,
            "search_ms_p95": round(percentile(latencies, 0.95), 3),
            "fresh_index_ms": round(index_ms, 3),
            "index_documents": indexed.get("documents"),
            "synthetic_sensitive_candidates_filtered": indexed.get("excluded_sensitive"),
            "process_max_rss_mib": round(rss / (1024 * 1024), 2),
            "privacy": "synthetic data only; temporary local index; aggregate-only output",
        }
        print(json.dumps(report, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
