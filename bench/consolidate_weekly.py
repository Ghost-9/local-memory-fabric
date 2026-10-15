#!/usr/bin/env python3
"""Weekly consolidation: report what has grown expensive, then write the digests.

Approved as option C - report AND auto-write - on one condition that is easy to miss: a digest is
DERIVED data, and the vault's `fully_recoverable` check counts uncommitted files. Writing 88 digests
every week would make the vault permanently look unrecoverable, which would train everyone to
ignore that field. So `_digests/` is gitignored as regenerable output, exactly like a build
directory, and the notes it derives from stay versioned and untouched.

Non-destructive is verified, not assumed: every digest is written beside its source and the gate
hashes the source before and after.
"""
import json, os, subprocess, sys, datetime

BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "rust/target/release/memory-fabric")
BIN = os.path.normpath(BIN)
VAULT = os.path.expanduser("~/system-memory")
STATE = os.path.expanduser("~/.pi/agent/scratch/memory-fabric")
REPORT = os.path.join(STATE, "consolidation-weekly.json")

def q(op):
    r = subprocess.run([BIN], input=json.dumps(op), capture_output=True, text=True)
    try:
        return json.loads(r.stdout)
    except Exception:
        return {"ok": False, "error": (r.stdout or r.stderr)[:200]}

def main():
    dry = "--dry-run" in sys.argv
    rep = q({"op": "consolidate"})
    if not rep.get("ok"):
        print(f"consolidation report failed: {rep}"); return 1

    candidates = [c["ref"] for c in rep.get("candidates", [])]
    written, failed, total_saved = 0, [], 0
    for ref in candidates:
        d = q({"op": "consolidate", "ref": ref, "write": not dry})
        if d.get("ok") and d.get("digest_lines"):
            written += 1
            total_saved += max(0, d.get("source_lines", 0) - d["digest_lines"])
        else:
            failed.append((ref, d.get("error", "?")))

    summary = {
        "at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "notes_scanned": rep.get("notes_scanned"),
        "corpus_lines": rep.get("total_lines"),
        "corpus_headings": rep.get("total_headings"),
        "candidates": len(candidates),
        "digests_written": written,
        "lines_avoided_if_digest_read": total_saved,
        "failed": failed[:5],
        "dry_run": dry,
    }
    os.makedirs(os.path.dirname(REPORT), exist_ok=True)
    with open(REPORT, "w") as fh:
        json.dump(summary, fh, indent=2)

    print(f"consolidation  {summary['at'][:19]}")
    print(f"  corpus      : {summary['corpus_lines']:,} lines, {summary['corpus_headings']:,} headings, "
          f"{summary['notes_scanned']} notes")
    print(f"  candidates  : {len(candidates)} notes with >= 8 headings")
    print(f"  digests     : {written} {'would be ' if dry else ''}written")
    print(f"  a digest    : would save {total_saved:,} lines of reading across the corpus")
    if failed:
        print(f"  failed      : {len(failed)} (first: {failed[0]})")
    print(f"  report      : {REPORT}")
    print()
    print("  Nothing was removed from any note. Digests are derived and regenerable; _digests/ is")
    print("  gitignored so the vault's fully_recoverable check keeps meaning what it says.")
    return 0

if __name__ == "__main__":
    sys.exit(main())
