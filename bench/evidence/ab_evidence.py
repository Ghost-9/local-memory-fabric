#!/usr/bin/env python3
"""Paired A/B with the discipline both competitors use and we did not.

This runner refuses to score a run that was not pre-registered. That refusal is the whole design:
the only thing separating a measurement from a story is that the analysis was fixed before the data
existed, and no amount of statistics applied afterwards can substitute for it.

It also refuses to drop pairs, and it reports the guard metric (task success) alongside the primary
one (cost), because a saving that costs correctness is a regression wearing a number.

Usage:
  python3 bench/evidence/ab_evidence.py validate <prereg.json>
  python3 bench/evidence/ab_evidence.py score <prereg.json> <results.json>
"""
import json, statistics, sys

def validate(path):
    p = json.load(open(path))
    problems = []
    if p.get("pairs", 0) < 6:
        problems.append(f"pairs={p.get('pairs')} - below 6 the interval is wider than any effect "
                        "worth acting on")
    if not p.get("tasks"):
        problems.append("no tasks declared")
    for t in p.get("tasks", []):
        if not t.get("success_check"):
            problems.append(f"task {t.get('id')} has no success check - cost alone cannot show "
                            "whether the answers got worse")
    if p.get("treatment") and p.get("arms", {}).get("treatment") is None and p.get("arms") == "auto":
        problems.append("arms not separated")
    if not p.get("stop_condition"):
        problems.append("no stop condition - 'run until it looks convincing' is not one")
    if "alternat" not in str(p.get("order", "")).lower():
        problems.append("pair order is not alternated, so provider drift is not cancelled")
    for x in problems:
        print(f"  REJECT: {x}", file=sys.stderr)
    if problems:
        print("\nThis run may not be scored. Fix the pre-registration first.", file=sys.stderr)
        return 1
    print(f"  pre-registration accepted: {p['id']}, {len(p['tasks'])} task(s), {p['pairs']} pairs")
    return 0

def score(prereg_path, results_path):
    # Refuse to score an unregistered run.
    if validate(prereg_path) != 0:
        return 1
    pre = json.load(open(prereg_path))
    res = json.load(open(results_path))
    rows = res.get("pairs", [])
    if len(rows) != pre["pairs"]:
        print(f"  REJECT: {len(rows)} pairs scored against a declared {pre['pairs']}. Dropping "
              "pairs is how a null result becomes a headline.", file=sys.stderr)
        return 1
    diffs, wins, success_ok = [], 0, True
    for r in rows:
        c, t = r["control"]["cost"], r["treatment"]["cost"]
        diffs.append(c - t)
        if t < c:
            wins += 1
        if r["control"]["success"] != r["treatment"]["success"]:
            success_ok = False
    mean_diff = statistics.mean(diffs)
    mean_control = statistics.mean(r["control"]["cost"] for r in rows)
    saved = mean_diff / mean_control if mean_control else 0.0
    n = len(diffs)
    if n > 1:
        sd = statistics.stdev(diffs)
        se = sd / (n ** 0.5)
        # t for 95% two-sided, small samples
        tcrit = {5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262,
                 10: 2.228, 12: 2.179, 15: 2.131, 20: 2.086}.get(n, 1.96)
        lo, hi = saved - tcrit * se / mean_control, saved + tcrit * se / mean_control
    else:
        lo = hi = saved
    print(f"  saved {saved*100:+.1f}%  (likely {lo*100:+.1f}% to {hi*100:+.1f}%), n={n}, wins={wins}/{n}")
    if not success_ok:
        print("  GUARD FAILED: task success differed between arms - a cost saving with a success "
              "loss is a regression, not a result", file=sys.stderr)
        return 1
    if lo < 0 < hi:
        print("  the interval includes zero - this is not a finding yet, whatever the mean says")
        return 0
    print("  interval excludes zero, and task success held in both arms")
    return 0

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(__doc__); sys.exit(1)
    mode = sys.argv[1]
    if mode == "validate":
        sys.exit(validate(sys.argv[2]))
    if mode == "score":
        sys.exit(score(sys.argv[2], sys.argv[3]))
    print(__doc__); sys.exit(1)
