#!/usr/bin/env python3
"""Aggregate-only Python/Rust benchmark on a reproducible synthetic vault."""
from __future__ import annotations

import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from benchmarks.synthetic import create_synthetic_vault  # noqa: E402
from memory_fabric import MemoryFabric  # noqa: E402

BINARY = Path(__file__).resolve().parent / "target" / "release" / "memory-fabric"


def percentile(values: list[float], p: float) -> float:
    xs = sorted(values)
    return xs[min(len(xs) - 1, int((len(xs) - 1) * p + 0.999999))] if xs else 0.0


def metrics(results: list[list[str]], cases: list[dict[str, object]]) -> tuple[float, float, int]:
    positive = [case for case in cases if case["expected"]]
    negative = [case for case in cases if not case["expected"]]
    hits = 0
    rr = 0.0
    false_positives = 0
    for case, refs in zip(cases, results):
        expected = set(case["expected"])
        if expected:
            rank = next((i + 1 for i, ref in enumerate(refs) if ref in expected), None)
            if rank:
                hits += 1
                rr += 1 / rank
        elif refs:
            false_positives += 1
    return (hits / len(positive) if positive else 0.0,
            rr / len(positive) if positive else 0.0,
            false_positives)


def main() -> int:
    if not BINARY.exists():
        print(json.dumps({"ok": False, "error": "Build rust/ with cargo build --release --locked first."}))
        return 2

    with tempfile.TemporaryDirectory(prefix="memory-fabric-benchmark-") as temp:
        base = Path(temp)
        vault = base / "synthetic-vault"
        cases = create_synthetic_vault(vault)
        python_home = base / "python-state"
        python_engine = MemoryFabric(home=python_home, vault=vault)
        python_env = os.environ.copy()
        python_env["MEMORY_FABRIC_HOME"] = str(python_home)
        python_env["MEMORY_FABRIC_VAULT"] = str(vault)
        start = time.perf_counter()
        python_index = python_engine.index()
        python_index_ms = (time.perf_counter() - start) * 1000
        py_results: list[list[str]] = []
        py_latency: list[float] = []
        py_spawn_latency: list[float] = []
        py_spawn_errors = 0
        py_spawn_mismatches = 0
        for case in cases:
            start = time.perf_counter()
            value = python_engine.search(str(case["query"]), provider="ollama", limit=5)
            py_latency.append((time.perf_counter() - start) * 1000)
            refs = [item["source_ref"] for item in value.get("results", [])]
            py_results.append(refs)
            request = json.dumps({"op": "search", "query": case["query"], "scope": "all", "limit": 5, "provider": "ollama"})
            start = time.perf_counter()
            process = subprocess.run([sys.executable, str(ROOT / "memory_fabric.py"), "--json-stdin"],
                                     input=request, text=True, capture_output=True, timeout=10, env=python_env)
            py_spawn_latency.append((time.perf_counter() - start) * 1000)
            try:
                spawned = json.loads(process.stdout)
                if process.returncode != 0 or not spawned.get("ok"):
                    py_spawn_errors += 1
                elif [item["source_ref"] for item in spawned.get("results", [])] != refs:
                    py_spawn_mismatches += 1
            except (ValueError, TypeError):
                py_spawn_errors += 1

        env = os.environ.copy()
        env["MEMORY_FABRIC_HOME"] = str(base / "rust-state")
        env["MEMORY_FABRIC_VAULT"] = str(vault)
        env["PI_MEMORY_LOCAL_PROVIDERS"] = "lmstudio,llama.cpp,ollama"
        proc = subprocess.Popen([str(BINARY), "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, text=True, env=env)

        def request(value: dict[str, object]) -> tuple[dict, float]:
            start = time.perf_counter()
            proc.stdin.write(json.dumps(value, ensure_ascii=False) + "\n")
            proc.stdin.flush()
            line = proc.stdout.readline()
            elapsed = (time.perf_counter() - start) * 1000
            return json.loads(line), elapsed

        try:
            _, cold_start_ms = request({"op": "status"})
            rust_index, rust_index_ms = request({"op": "index"})
            if not rust_index.get("ok"):
                print(json.dumps({"ok": False, "error": "Rust indexing failed."}))
                return 1
            rust_results: list[list[str]] = []
            rust_latency: list[float] = []
            for case in cases:
                value, elapsed = request({"op": "search", "query": case["query"], "scope": "all", "limit": 5, "provider": "ollama"})
                rust_latency.append(elapsed)
                rust_results.append([item["source_ref"] for item in value.get("results", [])])
            py_recall, py_mrr, py_false = metrics(py_results, cases)
            rust_recall, rust_mrr, rust_false = metrics(rust_results, cases)
            report = {
                "dataset": "synthetic-111-indexed-documents",
                "fixture_cases": len(cases),
                "positive_cases": sum(bool(c["expected"]) for c in cases),
                "negative_cases": sum(not c["expected"] for c in cases),
                "python_recall_at_5": round(py_recall, 4), "rust_recall_at_5": round(rust_recall, 4),
                "python_mrr_at_5": round(py_mrr, 4), "rust_mrr_at_5": round(rust_mrr, 4),
                "python_negative_false_positives": py_false, "rust_negative_false_positives": rust_false,
                "result_order_mismatches": sum(a != b for a, b in zip(py_results, rust_results)),
                "python_search_ms_p50": round(statistics.median(py_latency), 3),
                "python_search_ms_p95": round(percentile(py_latency, 0.95), 3),
                "python_spawned_api_ms_p50": round(statistics.median(py_spawn_latency), 3),
                "python_spawned_api_ms_p95": round(percentile(py_spawn_latency, 0.95), 3),
                "python_spawn_protocol_errors": py_spawn_errors,
                "python_spawn_result_mismatches": py_spawn_mismatches,
                "rust_persistent_api_ms_p50": round(statistics.median(rust_latency), 3),
                "rust_persistent_api_ms_p95": round(percentile(rust_latency, 0.95), 3),
                "python_fresh_index_ms": round(python_index_ms, 3),
                "rust_cold_start_status_ms": round(cold_start_ms, 3),
                "rust_fresh_index_ms": round(rust_index_ms, 3),
                "python_index_documents": python_index.get("documents"),
                "rust_index_documents": rust_index.get("documents"),
                "synthetic_sensitive_candidates_filtered": rust_index.get("excluded_sensitive"),
                "privacy": "synthetic data only; temporary local indexes; aggregate-only output",
            }
            rss_result = subprocess.run(["ps", "-o", "rss=", "-p", str(proc.pid)],
                                        capture_output=True, text=True, check=False)
            try:
                report["rust_worker_rss_sample_mib"] = round(float(rss_result.stdout.strip()) / 1024, 2)
            except ValueError:
                report["rust_worker_rss_sample_mib"] = None
        finally:
            proc.stdin.close()
            proc.wait(timeout=10)
        print(json.dumps(report, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
