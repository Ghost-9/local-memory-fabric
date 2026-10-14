#!/usr/bin/env python3
"""Cold-path A/B. Unique query suffixes defeat the repeat-query cache, so this measures
work rather than a warm replay. That distinction is the whole point: the previous harness
replayed the same probe terms and reported 1.22 ms while true cold p50 is 3.7-16 ms."""
import json, os, statistics, subprocess, sys, time
BASE_BIN, NEW_BIN = sys.argv[1], sys.argv[2]
TERMS = ["retention", "memory", "policy", "cache"]
def arm(binary, terms, limit, reps):
    env = dict(os.environ, PI_MEMORY_LOCAL_PROVIDERS="lmstudio,llama.cpp,ollama")
    p = subprocess.Popen([binary, "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.DEVNULL, text=True, env=env)
    lat = []
    try:
        for r in range(reps):
            for t in terms:
                # unique suffix -> cache miss every time
                q = f"{t} cold{r}{time.perf_counter_ns()}"
                s = time.perf_counter()
                p.stdin.write(json.dumps({"op": "search", "query": q, "scope": "all",
                                          "limit": limit, "provider": "llama.cpp"}) + "\n")
                p.stdin.flush()
                resp = json.loads(p.stdout.readline())
                lat.append((time.perf_counter() - s) * 1000)
                assert resp.get("ok"), resp.get("error")
    finally:
        p.stdin.close(); p.wait(timeout=10)
    return lat
def p50(xs): return statistics.median(xs)
print(f"{'limit':>6} {'arm':>8} {'n':>5} {'p50 ms':>9} {'p95 ms':>9}")
out = {}
for limit in (1, 10):
    res = {}
    for arm_name, binary in (("base", BASE_BIN), ("new", NEW_BIN)):
        a, b = None, None
        xs_all = []
        for rnd in range(4):                     # alternating to cancel drift
            for nm, binp in (("base", BASE_BIN), ("new", NEW_BIN)):
                xs = arm(binp, TERMS, limit, 6)
                if nm == arm_name: xs_all += xs
        res[arm_name] = xs_all
        s = sorted(xs_all)
        print(f"{limit:>6} {arm_name:>8} {len(s):>5} {p50(s):>9.2f} {s[int(0.95*len(s))]:>9.2f}")
    b, n = p50(res["base"]), p50(res["new"])
    out[limit] = (b, n, 100 * (n - b) / b)
    print(f"{'':>6} {'delta':>8} {'':>5} {100*(n-b)/b:>+8.1f}%")
print()
for limit, (b, n, d) in out.items():
    print(f"  limit={limit:<3} {b:.2f} -> {n:.2f} ms  ({d:+.1f}%)")
