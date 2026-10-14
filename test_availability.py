#!/usr/bin/env python3
"""A1 (degrade without the index) and C2 (detect and repair corruption).

Both use the real index, so both are dangerous to leave half-done: the fixture is copied aside
and restored in a finally block, and anything quarantined during the run is removed afterwards.

The point of each assertion is that it can fail:
  - A1 must return results AND say `degraded`, not silently pretend to be the real search.
  - A1 must abstain when no term discriminates, because a search that cannot say "nothing"
    is worse than a slow one.
  - C2 must refuse to repair a healthy index (a rebuild on a transient error discards good data).
  - C2 must quarantine rather than delete, so the fault stays diagnosable.
  - status must answer while the index is unreadable - that is the moment it is read.
"""
import glob, json, os, shutil, subprocess, sys

BIN = "rust/target/release/memory-fabric"
HOME = os.path.expanduser("~/.pi/agent/scratch/memory-fabric")
IDX = os.path.join(HOME, "index.sqlite3")
fails = []

def q(op):
    r = subprocess.run([BIN], input=json.dumps(op), capture_output=True, text=True)
    try:
        return json.loads(r.stdout)
    except Exception:
        return {"_unparseable": (r.stdout or r.stderr)[:200]}

def S(**kw):
    return q({"op": "search", "scope": "all", "limit": 3, "provider": "pi", "loopback": True, **kw})

def damaged_bytes(offset, n=80):
    with open(IDX, "r+b") as f:
        f.seek(offset); f.write(b"\xaa" * n)

if not os.path.exists(IDX):
    print("  no index to test against; skipping (the gate indexes first)"); sys.exit(0)

backup = IDX + ".availability-test"
shutil.copy2(IDX, backup)
try:
    # ---- C2: a healthy index must not be rebuilt ----
    r = q({"op": "repair"})
    if r.get("repaired"):
        fails.append("C2 repaired a healthy index - a rebuild on a transient error discards good data")
    if r.get("integrity") != "ok":
        fails.append(f"C2 reported integrity {r.get('integrity')!r} on a healthy index")
    print(f"  healthy index : repaired={r.get('repaired')} integrity={r.get('integrity')!r}")

    # ---- A1 + C2: damage the header (page 1), the shape that makes every query fail ----
    damaged_bytes(24)
    st = q({"op": "status"})
    if st.get("integrity_ok") is not False:
        fails.append("status did not flag a damaged index")
    if st.get("readable") is not False:
        fails.append("status did not report the index unreadable")
    print(f"  damaged index : integrity={str(st.get('integrity'))[:40]!r} readable={st.get('readable')} documents={st.get('documents')}")

    d = S(query="decision log")
    if not d.get("degraded"):
        fails.append("A1 did not mark the result degraded when the index was unreadable")
    if not d.get("results"):
        fails.append("A1 returned nothing when the index was unreadable - it should still answer")
    print(f"  A1 degraded   : results={len(d.get('results', []))} degraded={d.get('degraded')} top={(d.get('results') or [{}])[0].get('relpath')}")

    z = S(query="the and of a")
    if z.get("results"):
        fails.append(f"A1 returned {len(z['results'])} results for a query with no discriminating term - it must abstain")
    print(f"  A1 abstains   : results={len(z.get('results', []))} abstention={z.get('abstention')!r}")

    rp = q({"op": "repair"})
    if not rp.get("repaired"):
        fails.append("C2 did not repair a genuinely unreadable index")
    if not rp.get("quarantined"):
        fails.append("C2 did not quarantine the unusable file - the fault would be undiagnosable")
    print(f"  C2 repaired   : quarantined={rp.get('quarantined')}")

    st2 = q({"op": "status"})
    if st2.get("integrity") != "ok":
        fails.append(f"index still unhealthy after repair: {st2.get('integrity')!r}")
    if not S(query="decision log").get("results"):
        fails.append("search returned nothing after a repair")
    print(f"  after repair  : integrity={st2.get('integrity')!r} documents={st2.get('documents')}")
finally:
    for f in glob.glob(IDX + ".corrupt-*") + glob.glob(IDX + "-wal.corrupt-*") + glob.glob(IDX + "-shm.corrupt-*"):
        os.remove(f)
    for junk in [IDX, IDX + "-wal", IDX + "-shm"]:
        if os.path.exists(junk):
            os.remove(junk)
    shutil.copy2(backup, IDX)
    os.remove(backup)

if fails:
    for f in fails:
        print(f"  FAIL: {f}", file=sys.stderr)
    sys.exit(1)
print("  OK: A1 degrades and abstains correctly; C2 detects, quarantines and rebuilds, and leaves healthy indexes alone")
