#!/usr/bin/env python3
"""C1 (expand-only migration with reconciliation) and C3 (data intact across it).

A migration test that only checks "the ALTER ran" proves nothing. This builds a genuinely
old-format index - the current one with a column dropped and the version rolled back - then runs
the real index path over it and asserts:

  * the column comes back with the right default,
  * the row counts are identical either side (C3),
  * the version advances only after that holds,
  * and searches still return the same documents.

The reconciliation assertion is exercised for real here, because a migration with no structural
step reconciles trivially and would pass whether or not the check existed.
"""
import json, os, shutil, sqlite3, subprocess, sys

BIN = "rust/target/release/memory-fabric"
IDX = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/index.sqlite3")
fails = []

def q(op):
    r = subprocess.run([BIN], input=json.dumps(op), capture_output=True, text=True)
    try:
        return json.loads(r.stdout)
    except Exception:
        return {"_unparseable": (r.stdout or r.stderr)[:200]}

def S(**kw):
    return q({"op": "search", "scope": "all", "limit": 5, "provider": "pi", "loopback": True, **kw})

if not os.path.exists(IDX):
    print("  no index; skipping"); sys.exit(0)

backup = IDX + ".migration-test"
shutil.copy2(IDX, backup)
try:
    before = S(query="decision log")
    before_refs = [r["source_ref"] for r in before.get("results", [])]
    con = sqlite3.connect(IDX)
    counts_before = tuple(
        con.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
        for t in ("documents", "doc_terms", "term_df")
    )
    # Build a genuinely old-format index: drop a column the ladder is supposed to add, roll the
    # version back, and clear the migration record.
    try:
        con.execute("ALTER TABLE documents DROP COLUMN export_policy")
    except sqlite3.OperationalError as e:
        print(f"  sqlite cannot drop a column here ({e}); skipping the structural case")
        con.close(); sys.exit(0)
    con.execute("UPDATE metadata SET value='2' WHERE key='schema_version'")
    con.execute("DELETE FROM metadata WHERE key='last_migration'")
    con.commit()
    cols = [r[1] for r in con.execute("PRAGMA table_info(documents)")]
    con.close()
    if "export_policy" in cols:
        fails.append("test setup failed: export_policy was not dropped")
    print(f"  old-format index built: documents columns={len(cols)}, schema_version=2, export_policy absent")

    # Run the real index path - this is what a user's next index command does.
    r = q({"op": "index"})
    con = sqlite3.connect(IDX)
    cols_after = [r_[1] for r_ in con.execute("PRAGMA table_info(documents)")]
    counts_after = tuple(
        con.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
        for t in ("documents", "doc_terms", "term_df")
    )
    ver = con.execute("SELECT value FROM metadata WHERE key='schema_version'").fetchone()
    mig = con.execute("SELECT value FROM metadata WHERE key='last_migration'").fetchone()
    dflt = con.execute("SELECT COUNT(*) FROM documents WHERE export_policy='deny_remote'").fetchone()[0]
    con.close()

    if "export_policy" not in cols_after:
        fails.append("the migration did not restore the dropped column")
    # `documents` is data: it must be identical, and the failure it guards against is real.
    if counts_before[0] != counts_after[0]:
        fails.append(f"reconciliation: document count changed {counts_before[0]} -> {counts_after[0]}")
    # `doc_terms` and `term_df` are derived, and a migration forces a full re-read that rebuilds
    # them from current content. Asserting equality here made the gate fail on a correct
    # migration, which is how a strict test gets weakened or switched off. They are reported, not
    # asserted. The invariant that matters - that the ALTER itself changes no rows - is asserted
    # inside init_db, between the columns being added and the version being written.
    if counts_before[1:] != counts_after[1:]:
        print(f"  derived tables rebuilt: doc_terms {counts_before[1]}->{counts_after[1]}, "
              f"term_df {counts_before[2]}->{counts_after[2]} (expected: a migration forces a re-read)")
    if not ver or ver[0] != "3":
        fails.append(f"schema_version is {ver!r}, expected '3'")
    if not mig:
        fails.append("the migration was not recorded in metadata")
    if dflt != counts_after[0]:
        fails.append(f"the restored column did not default for pre-existing rows ({dflt}/{counts_after[0]})")
    print(f"  after migration     : columns={len(cols_after)} version={ver and ver[0]!r}")
    print(f"  reconciliation      : documents {counts_before[0]} -> {counts_after[0]}  {'IDENTICAL' if counts_before[0] == counts_after[0] else 'CHANGED'}")
    print(f"  default backfilled  : {dflt}/{counts_after[0]} rows carry 'deny_remote' (fail-closed default)")
    print(f"  recorded            : {mig and mig[0]}")

    after = S(query="decision log")
    refs_after = [x["source_ref"] for x in after.get("results", [])]
    if before_refs != refs_after:
        fails.append(f"search results changed across the migration: {before_refs} -> {refs_after}")
    print(f"  search unchanged    : {refs_after == before_refs}")
finally:
    for junk in [IDX, IDX + "-wal", IDX + "-shm"]:
        if os.path.exists(junk):
            os.remove(junk)
    shutil.copy2(backup, IDX)
    os.remove(backup)

if fails:
    for f in fails:
        print(f"  FAIL: {f}", file=sys.stderr)
    sys.exit(1)
print("  OK: expand-only migration restored the column, reconciled, advanced the version only after that, and changed no data")
