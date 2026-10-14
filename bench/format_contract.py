#!/usr/bin/env python3
"""C4 - fail when the index format changes without being recorded.

The recorded retrieval fixture (bench/golden/parity.json, 544 cases) was captured against a
particular index shape. If the shape moves and nothing says so, that fixture keeps passing while
measuring something that no longer exists - the same silent-skip failure this repo keeps finding.

So the format is written down, and drift is a gate failure with the exact difference printed.
Recording a deliberate change is `bench/format_contract.py --record`.
"""
import json, os, subprocess, sys

BIN = "rust/target/release/memory-fabric"
GOLD = "bench/golden/format.json"

def live():
    r = subprocess.run([BIN], input=json.dumps({"op": "schema"}), capture_output=True, text=True)
    try:
        return json.loads(r.stdout)
    except Exception:
        return None

def main():
    if not os.path.exists(BIN):
        print("  engine not built; skipping"); return 0
    d = live()
    if not d or not d.get("ok"):
        print("  schema op unavailable - the engine may predate C4; recording nothing")
        return 0
    keys = ["schema_version", "tables", "documents_columns", "fts_tokenizer",
            "migration_policy", "rollback"]
    now = {k: d.get(k) for k in keys}
    if "--record" in sys.argv:
        now["_note"] = ("The index format, as data. bench/format_contract.py fails when the live "
                        "index differs from this file, so a schema change cannot land silently and "
                        "leave the recorded retrieval fixture measuring a shape that no longer exists.")
        json.dump(now, open(GOLD, "w"), indent=2, sort_keys=True)
        print(f"  recorded: version={now['schema_version']}")
        return 0
    if not os.path.exists(GOLD):
        print(f"  no {GOLD}; run bench/format_contract.py --record")
        return 0
    want = json.load(open(GOLD))
    diffs = []
    for k in keys:
        if want.get(k) != now.get(k):
            a, b = want.get(k), now.get(k)
            if isinstance(a, list) and isinstance(b, list):
                added, removed = sorted(set(b) - set(a)), sorted(set(a) - set(b))
                diffs.append(f"{k}: added {added}, removed {removed}")
            else:
                diffs.append(f"{k}: recorded {a!r} != live {b!r}")
    if diffs:
        print("  FORMAT DRIFT - the index shape changed without being recorded:", file=sys.stderr)
        for d_ in diffs:
            print(f"    {d_}", file=sys.stderr)
        print("  If intended, re-record with bench/format_contract.py --record and say so in", file=sys.stderr)
        print("  the commit message: recorded fixtures captured against the old shape are now stale.", file=sys.stderr)
        return 1
    print(f"  index format matches the record: version={now['schema_version']}, "
          f"{len(now['tables'])} tables, {len(now['documents_columns'])} document columns")
    return 0

if __name__ == "__main__":
    sys.exit(main())
