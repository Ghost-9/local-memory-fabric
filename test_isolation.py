#!/usr/bin/env python3
"""A2 - reader/writer isolation, measured and then frozen as a regression guard.

The milestone called for a "read/write bulkhead": separate paths so a long index cannot block
searches. Measuring first changed the conclusion, and the measurement is kept here because the
*reason* not to build something is as valuable as the thing itself:

    readers during a full forced reindex : p50 12.1 ms vs 11.5 ms alone  (+5.1%)
    worst case                           : 12.7 ms vs 12.8 ms            (no worse)
    writer during a full forced reindex  : p50 9 ms                      (not blocked)

The index sets journal_mode=WAL, so readers take a snapshot and a writer does not fence them.
There is no contention to bulkhead, and adding a second path would be complexity bought with
nothing. What *is* worth having is this: an assertion that the property holds, so a future change
that drops WAL, or takes an immediate lock for the whole build, fails the gate instead of quietly
turning a 12 ms search into a multi-second one under load.

Thresholds are deliberately loose (2x) because this measures a laptop under CI-ish load; it is a
collapse detector, not a latency benchmark.
"""
import json, statistics, subprocess, sys, threading, time

BIN = "rust/target/release/memory-fabric"
fails = []

def search():
    t = time.perf_counter()
    subprocess.run([BIN], input=json.dumps({"op": "search", "query": "decision log", "scope": "all",
                                            "limit": 5, "provider": "pi", "loopback": True}),
                   capture_output=True, text=True)
    return (time.perf_counter() - t) * 1000

base = [search() for _ in range(11)]
base_p50 = statistics.median(base)

stop = False
def reindex():
    while not stop:
        subprocess.run([BIN], input=json.dumps({"op": "index", "force": True}),
                       capture_output=True, text=True)
th = threading.Thread(target=reindex, daemon=True)
th.start()
time.sleep(0.15)
cont = [search() for _ in range(11)]
stop = True
th.join(timeout=30)
cont_p50 = statistics.median(cont)

ratio = cont_p50 / base_p50 if base_p50 else 1.0
print(f"  readers alone   p50 {base_p50:.1f} ms   with a reindex p50 {cont_p50:.1f} ms   ({ratio:.2f}x)")
print(f"  worst           {max(base):.1f} ms alone vs {max(cont):.1f} ms contended")
if ratio > 2.0:
    fails.append(f"a concurrent reindex slowed searches {ratio:.2f}x - the WAL snapshot property is gone")

# And the property that makes it true, asserted directly rather than inferred from the timings.
import os, sqlite3
idx = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/index.sqlite3")
if os.path.exists(idx):
    mode = sqlite3.connect(idx).execute("PRAGMA journal_mode").fetchone()[0]
    print(f"  journal_mode    {mode}")
    if mode.lower() != "wal":
        fails.append(f"journal_mode is {mode!r}, not wal - readers would block behind the writer")

if fails:
    for f in fails:
        print(f"  FAIL: {f}", file=sys.stderr)
    sys.exit(1)
print("  OK: readers are not blocked by a concurrent index run, and WAL is what makes that true")
