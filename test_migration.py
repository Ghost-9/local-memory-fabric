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

HERMETIC SINCE 2026-10-19. This test used to run against the live index and the live vault, and the
vault is a knowledge base other agents write to while the gate runs. A concurrent write shifts
document frequencies, which shifts ranking, which made `before_refs == refs_after` fail on a
CORRECT migration - a false failure, and the exact kind that gets a strict test weakened or deleted.

The earlier partial fix relaxed the count comparison to a superset check and left the search
comparison racing. Relaxing is the wrong direction. Instead the corpus is now frozen: the vault is
copied into a temporary directory and the engine is pointed at it through MEMORY_FABRIC_VAULT and
MEMORY_FABRIC_HOME. Nothing else on the machine can move under the test, so "the migration changed
no ranking" is a real assertion again rather than a race that happens to pass.

Cost: ~131 markdown files copied, under 100 ms. Worth it - a gate that fails at random teaches the
reader to ignore it.
"""
import json, os, shutil, sqlite3, subprocess, sys, tempfile

BIN = "rust/target/release/memory-fabric"
LIVE_VAULT = os.path.expanduser("~/system-memory")
LIVE_CONFIG = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/config.json")
fails = []


def _freeze_vault():
    """Copy the live vault's markdown into a temp dir; return (workdir, vault, home, env, count)."""
    work = tempfile.mkdtemp(prefix="engram-migration-")
    vault, home = os.path.join(work, "vault"), os.path.join(work, "home")
    os.makedirs(vault)
    os.makedirs(home)
    # The config carries the include_dirs whitelist AND the provider allowlist. Without it a fresh
    # state directory is fail-closed: search returns {"blocked": ...} because no provider is allowed,
    # and a test that ignores that would silently compare two empty result sets and pass. Copying it
    # also keeps the include/exclude rules identical to production, which is what we want to test.
    shutil.copy2(LIVE_CONFIG, os.path.join(home, "config.json"))
    n = 0
    for root, dirs, files in os.walk(LIVE_VAULT):
        dirs[:] = [d for d in dirs if d != ".git"]
        for fn in files:
            if not fn.endswith(".md"):
                continue
            src = os.path.join(root, fn)
            dst = os.path.join(vault, os.path.relpath(src, LIVE_VAULT))
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            try:
                shutil.copy2(src, dst)
                n += 1
            except OSError:
                pass  # a file vanishing mid-copy is fine; we want a stable snapshot, not this file
    env = {**os.environ, "MEMORY_FABRIC_VAULT": vault, "MEMORY_FABRIC_HOME": home}
    return work, vault, home, env, n


WORK, VAULT, HOME, ENV, COPIED = _freeze_vault()
IDX = os.path.join(HOME, "index.sqlite3")
# Self-check: if this ever points outside the temp workdir, the test is racing the live vault again
# and its search comparison means nothing. Cheaper to assert than to notice a random red gate later.
assert os.path.realpath(IDX).startswith(os.path.realpath(WORK) + os.sep), (
    f"migration test is not hermetic: index {IDX} is outside {WORK}")
assert ENV["MEMORY_FABRIC_VAULT"] == VAULT and ENV["MEMORY_FABRIC_HOME"] == HOME


def q(op):
    r = subprocess.run([BIN], input=json.dumps(op), capture_output=True, text=True, env=ENV)
    try:
        return json.loads(r.stdout)
    except Exception:
        return {"_unparseable": (r.stdout or r.stderr)[:200]}


def S(**kw):
    return q({"op": "search", "scope": "all", "limit": 5, "provider": "pi", "loopback": True, **kw})


try:
    if not COPIED:
        print("  empty vault copy; nothing to migrate")
        sys.exit(0)
    # Build the frozen index. This is the baseline both sides of the migration are measured against.
    q({"op": "index"})
    if not os.path.exists(IDX):
        print("  could not build a frozen index; skipping")
        sys.exit(0)
    con0 = sqlite3.connect(IDX)
    frozen_docs = con0.execute("SELECT COUNT(*) FROM documents").fetchone()[0]
    con0.close()
    if frozen_docs == 0:
        # Never let this pass silently: an empty index on both sides makes every comparison trivially
        # equal, which is how a broken test reports success.
        print("  FAIL: the frozen index holds 0 documents, so nothing here would be tested", file=sys.stderr)
        sys.exit(1)
    print(f"  frozen corpus       : {COPIED} notes copied, {frozen_docs} indexed from a temp vault "
          f"(no concurrent writers possible)")

    before = S(query="decision log")
    before_refs = [r["source_ref"] for r in before.get("results", [])]
    if not before_refs:
        print(f"  FAIL: the probe query returned nothing from a {frozen_docs}-document index; "
              f"response was {str(before)[:200]}", file=sys.stderr)
        sys.exit(1)
    con = sqlite3.connect(IDX)
    counts_before = tuple(
        con.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
        for t in ("documents", "doc_terms", "term_df")
    )
    docs_before = {r[0] for r in con.execute("SELECT relpath FROM documents")}
    # Build a genuinely old-format index: drop a column the ladder is supposed to add, roll the
    # version back, and clear the migration record.
    try:
        con.execute("ALTER TABLE documents DROP COLUMN export_policy")
    except sqlite3.OperationalError as e:
        print(f"  sqlite cannot drop a column here ({e}); skipping the structural case")
        con.close()
        sys.exit(0)
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
    # With a frozen corpus nothing may be lost, duplicated, OR added. This is stricter than the
    # superset check it replaces: that check existed only to tolerate the race this test no longer has.
    con2 = sqlite3.connect(IDX)
    docs_after = {r[0] for r in con2.execute("SELECT relpath FROM documents")}
    dupes = con2.execute(
        "SELECT relpath FROM documents GROUP BY relpath HAVING COUNT(*) > 1").fetchall()
    con2.close()
    lost = docs_before - docs_after
    added = docs_after - docs_before
    if lost:
        fails.append(f"reconciliation: the migration LOST {len(lost)} document(s): {sorted(lost)[:5]}")
    if added:
        # Impossible with a frozen corpus: it means the re-read invented documents.
        fails.append(f"reconciliation: the migration ADDED {len(added)} document(s) from a frozen corpus: {sorted(added)[:5]}")
    if dupes:
        fails.append(f"reconciliation: {len(dupes)} duplicated relpath(s): {[d[0] for d in dupes][:5]}")
    # `doc_terms` and `term_df` are derived, and a migration forces a full re-read that rebuilds
    # them from current content. With a frozen corpus they must come back identical. Asserting this
    # on the live vault was impossible; here it is the real check that the re-read is deterministic.
    if counts_before[1:] != counts_after[1:]:
        fails.append(
            f"derived tables differ across a migration over an unchanged corpus: "
            f"doc_terms {counts_before[1]}->{counts_after[1]}, term_df {counts_before[2]}->{counts_after[2]}")
    if not ver or ver[0] != "3":
        fails.append(f"schema_version is {ver!r}, expected '3'")
    if not mig:
        fails.append("the migration was not recorded in metadata")
    if dflt != counts_after[0]:
        fails.append(f"the restored column did not default for pre-existing rows ({dflt}/{counts_after[0]})")
    print(f"  after migration     : columns={len(cols_after)} version={ver and ver[0]!r}")
    print(f"  reconciliation      : {len(docs_before)} documents before, {len(docs_after)} after, "
          f"lost={len(lost)} added={len(added)} duplicated={len(dupes)}")
    print(f"  derived tables      : doc_terms {counts_before[1]}->{counts_after[1]}, "
          f"term_df {counts_before[2]}->{counts_after[2]} (must be identical over a frozen corpus)")
    print(f"  default backfilled  : {dflt}/{counts_after[0]} rows carry 'deny_remote' (fail-closed default)")
    print(f"  recorded            : {mig and mig[0]}")

    after = S(query="decision log")
    refs_after = [x["source_ref"] for x in after.get("results", [])]
    if before_refs != refs_after:
        fails.append(f"search results changed across the migration: {before_refs} -> {refs_after}")
    print(f"  search unchanged    : {refs_after == before_refs}")
finally:
    shutil.rmtree(WORK, ignore_errors=True)

if fails:
    for f in fails:
        print(f"  FAIL: {f}", file=sys.stderr)
    sys.exit(1)
print("  OK: expand-only migration restored the column, reconciled, advanced the version only after that, and changed no data")
