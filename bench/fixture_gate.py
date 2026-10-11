"""Gate step: assert the gold fixture has not regressed.

Kept as a file rather than an inline `python3 -c` so the thresholds are readable, diffable, and
reviewable — and so shell quoting cannot mangle the f-strings.

Floors, not equality. The measured values are well above these; the floors sit above the
pre-fixture state so a real regression fails, while ordinary variance does not make it flaky.
Re-derive the measured value with:  python3 bench/fixture_score.py
"""
from __future__ import annotations

import json
import sys

# Measured 2026-10-12 on this machine: recall@3 0.7459, mrr@3 0.7491, no-hit 0.9286.
FLOOR_RECALL_AT_3 = 0.70
FLOOR_NOHIT_PRECISION = 0.90


def main(path: str) -> int:
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)

    failures: list[str] = []
    for engine, result in data["results"].items():
        r3 = result["recall_at_3"]
        nh = result["nohit_precision"]
        if r3 < FLOOR_RECALL_AT_3:
            failures.append(f"{engine} recall@3 {r3} < {FLOOR_RECALL_AT_3}")
        if nh < FLOOR_NOHIT_PRECISION:
            failures.append(f"{engine} no-hit precision {nh} < {FLOOR_NOHIT_PRECISION}")

    py = data["results"].get("python", {})
    rs = data["results"].get("rust", {})
    print(f"  {data['cases']} cases | recall@3 {py.get('recall_at_3')} | "
          f"mrr@3 {py.get('mrr_at_3')} | no-hit {py.get('nohit_precision')} | "
          f"p50 {py.get('latency_p50_ms')}ms py / {rs.get('latency_p50_ms')}ms rust")

    if failures:
        print("GATE: gold fixture regression: " + "; ".join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else "/tmp/mf-fixture.json"))