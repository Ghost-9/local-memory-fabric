#!/usr/bin/env python3
"""C1/C3 - migrate a synthetic v3 index to external-content FTS without losing source rows.

The test reconstructs the old internal-content FTS shape in a temporary index, then runs the real
Rust migration and checks that document and term_df counts, headings, version metadata, and search
results survive. It also exercises edit/delete synchronization in both engines. The fixture is
generated locally; no operator vault or config is read.
"""
import json, os, shutil, sqlite3, subprocess, sys, tempfile

BIN = "rust/target/release/memory-fabric"
fails = []


def _freeze_vault():
    """Build a deterministic isolated corpus without reading the operator's vault."""
    work = tempfile.mkdtemp(prefix="engram-migration-")
    vault, home = os.path.join(work, "vault"), os.path.join(work, "home")
    os.makedirs(vault)
    os.makedirs(home)
    with open(os.path.join(home, "config.json"), "w", encoding="utf-8") as f:
        json.dump({"include_dirs": ["projects"], "include_files": [], "excluded_dirs": [],
                   "local_providers": ["pi"]}, f)
    os.makedirs(os.path.join(vault, "projects"))
    for name, body in {
        "decision-log.md": "# Decision Log\n\nMigration decision log marker.",
        "decision-log-archive.md": "# Decision Log Archive\n\nOlder decision log marker.",
        "other.md": "# Other\n\nUnrelated material.",
    }.items():
        with open(os.path.join(vault, "projects", name), "w", encoding="utf-8") as f:
            f.write(body + "\n")
    n = 3
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
    counts_before = tuple(con.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
                          for t in ("documents", "term_df"))
    docs_before = {r[0] for r in con.execute("SELECT relpath FROM documents")}
    # Reconstruct the genuine v3 shape: internal-content FTS and no headings column.
    try:
        for trigger in ("documents_fts_ai", "documents_fts_ad", "documents_fts_au"):
            con.execute(f"DROP TRIGGER IF EXISTS {trigger}")
        con.execute("CREATE VIRTUAL TABLE documents_fts_v3 USING fts5(relpath UNINDEXED,title,headings,body,tokenize='porter unicode61')")
        con.execute("INSERT INTO documents_fts_v3(rowid,relpath,title,headings,body) SELECT id,relpath,title,headings,body FROM documents")
        con.execute("DROP TABLE documents_fts")
        con.execute("ALTER TABLE documents_fts_v3 RENAME TO documents_fts")
        con.execute("ALTER TABLE documents DROP COLUMN headings")
    except sqlite3.OperationalError as e:
        print(f"  cannot construct v3 migration fixture ({e})", file=sys.stderr)
        con.close()
        sys.exit(1)
    con.execute("CREATE TABLE doc_terms(relpath TEXT NOT NULL,term TEXT NOT NULL,PRIMARY KEY(relpath,term)) WITHOUT ROWID")
    con.execute("CREATE INDEX doc_terms_by_term ON doc_terms(term)")
    con.execute("CREATE VIRTUAL TABLE temp._inst USING fts5vocab(main,documents_fts,'instance')")
    con.execute("INSERT OR IGNORE INTO doc_terms SELECT f.relpath,v.term FROM temp._inst v JOIN documents_fts f ON f.rowid=v.doc")
    con.execute("UPDATE metadata SET value='3' WHERE key='schema_version'")
    con.execute("DELETE FROM metadata WHERE key='last_migration'")
    con.commit()
    cols = [r[1] for r in con.execute("PRAGMA table_info(documents)")]
    con.close()
    if "headings" in cols:
        fails.append("test setup failed: headings was not dropped")
    print(f"  old-format index built: documents columns={len(cols)}, schema_version=3, internal-content FTS")

    # Run the real index path - this is what a user's next index command does.
    r = q({"op": "index"})
    con = sqlite3.connect(IDX)
    cols_after = [r_[1] for r_ in con.execute("PRAGMA table_info(documents)")]
    counts_after = tuple(con.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
                         for t in ("documents", "term_df"))
    ver = con.execute("SELECT value FROM metadata WHERE key='schema_version'").fetchone()
    mig = con.execute("SELECT value FROM metadata WHERE key='last_migration'").fetchone()
    fts_sql = con.execute("SELECT sql FROM sqlite_master WHERE name='documents_fts'").fetchone()[0]
    old_terms = con.execute("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='doc_terms'").fetchone()[0]
    dflt = con.execute("SELECT COUNT(*) FROM documents WHERE headings != ''").fetchone()[0]
    con.close()

    if "headings" not in cols_after:
        fails.append("the migration did not add documents.headings")
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
    if counts_before[1] != counts_after[1]:
        fails.append(f"term_df differs across an unchanged-corpus migration: {counts_before[1]}->{counts_after[1]}")
    if "content='documents'" not in fts_sql:
        fails.append("documents_fts is not external-content after migration")
    if old_terms:
        fails.append("doc_terms survived the migration")
    if not ver or ver[0] != "4":
        fails.append(f"schema_version is {ver!r}, expected '4'")
    if not mig:
        fails.append("the migration was not recorded in metadata")
    if dflt != counts_after[0]:
        fails.append(f"headings were not preserved for pre-existing rows ({dflt}/{counts_after[0]})")
    print(f"  after migration     : columns={len(cols_after)} version={ver and ver[0]!r}")
    print(f"  reconciliation      : {len(docs_before)} documents before, {len(docs_after)} after, "
          f"lost={len(lost)} added={len(added)} duplicated={len(dupes)}")
    print(f"  term_df             : {counts_before[1]}->{counts_after[1]} rows over unchanged corpus")
    print(f"  headings preserved  : {dflt}/{counts_after[0]} rows migrated")
    print(f"  recorded            : {mig and mig[0]}")

    after = S(query="decision log")
    refs_after = [x["source_ref"] for x in after.get("results", [])]
    if before_refs != refs_after:
        fails.append(f"search results changed across the migration: {before_refs} -> {refs_after}")
    print(f"  search unchanged    : {refs_after == before_refs}")

    # External-content FTS must stay synchronized for edits and deletes, using the source row's
    # old indexed values for the FTS5 delete command.
    con = sqlite3.connect(IDX)
    rel = "projects/other.md"
    original = con.execute("SELECT body FROM documents WHERE relpath=?", (rel,)).fetchone()[0]
    con.execute("UPDATE documents SET body=body || ' editprobeunique' WHERE relpath=?", (rel,))
    edit_hits = con.execute("SELECT COUNT(*) FROM documents_fts WHERE documents_fts MATCH 'editprobeunique'").fetchone()[0]
    con.execute("UPDATE documents SET body=? WHERE relpath=?", (original, rel))
    stale_hits = con.execute("SELECT COUNT(*) FROM documents_fts WHERE documents_fts MATCH 'editprobeunique'").fetchone()[0]
    con.execute("UPDATE documents SET body=body || ' deleteprobeunique' WHERE relpath=?", (rel,))
    con.execute("DELETE FROM documents WHERE relpath=?", (rel,))
    delete_hits = con.execute("SELECT COUNT(*) FROM documents_fts WHERE documents_fts MATCH 'deleteprobeunique'").fetchone()[0]
    con.close()
    if (edit_hits, stale_hits, delete_hits) != (1, 0, 0):
        fails.append(f"external FTS trigger sync failed: edit={edit_hits}, stale_after_edit={stale_hits}, after_delete={delete_hits}")
    print(f"  FTS edit/delete     : edit={edit_hits}, stale-after-edit={stale_hits}, after-delete={delete_hits}")

    # Exercise Python's real filesystem edit/delete path against its own isolated database too.
    py_home = os.path.join(WORK, "python-home")
    os.makedirs(py_home)
    with open(os.path.join(py_home, "config.json"), "w", encoding="utf-8") as f:
        json.dump({"include_dirs": ["projects"], "include_files": [], "excluded_dirs": [],
                   "local_providers": ["pi"]}, f)
    py_env = {**ENV, "MEMORY_FABRIC_HOME": py_home}
    py_bin = [sys.executable, "memory_fabric.py", "--json-stdin"]
    py_db = os.path.join(py_home, "index.sqlite3")
    py_index = subprocess.run(py_bin, input=json.dumps({"op": "index"}), capture_output=True,
                              text=True, env=py_env)
    if py_index.returncode:
        fails.append(f"Python initial index failed: {(py_index.stdout or py_index.stderr)[:200]}")
    source = os.path.join(VAULT, "projects", "other.md")
    with open(source, "w", encoding="utf-8") as f:
        f.write("# Other\n\npythonfreshprobeunique\n")
    py_update = subprocess.run(py_bin, input=json.dumps({"op": "index"}), capture_output=True,
                               text=True, env=py_env)
    py_con = sqlite3.connect(py_db)
    fresh_hits = py_con.execute("SELECT COUNT(*) FROM documents_fts WHERE documents_fts MATCH 'pythonfreshprobeunique'").fetchone()[0]
    stale_old = py_con.execute("SELECT COUNT(*) FROM documents_fts WHERE documents_fts MATCH 'unrelated'").fetchone()[0]
    os.remove(source)
    py_delete = subprocess.run(py_bin, input=json.dumps({"op": "index"}), capture_output=True,
                               text=True, env=py_env)
    after_delete = py_con.execute("SELECT COUNT(*) FROM documents_fts WHERE documents_fts MATCH 'pythonfreshprobeunique'").fetchone()[0]
    py_con.close()
    if py_update.returncode or py_delete.returncode or (fresh_hits, stale_old, after_delete) != (1, 0, 0):
        fails.append(f"Python external FTS edit/delete failed: edit={fresh_hits}, stale={stale_old}, after-delete={after_delete}")
    print(f"  Python edit/delete : edit={fresh_hits}, stale={stale_old}, after-delete={after_delete}")
finally:
    shutil.rmtree(WORK, ignore_errors=True)

if fails:
    for f in fails:
        print(f"  FAIL: {f}", file=sys.stderr)
    sys.exit(1)
print("  OK: expand-only migration restored the column, reconciled, advanced the version only after that, and changed no data")
