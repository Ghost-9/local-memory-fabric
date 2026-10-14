#!/usr/bin/env python3
"""#95 - what has memory actually served?

The record exists to answer "what has this served?" and required a Python one-liner to read. This
is that one-liner, made repeatable and honest about what it cannot know.

Two things it deliberately does NOT print:
  - a "read-through rate". The refs block tells the model which notes are relevant; whether it then
    reads them is not in this file, and a ratio invented here would be a number nobody could check.
  - a "fixation" percentage from citation counts. Counting [[wikilinks]] the *agent* wrote measures
    what the agent wrote, not what memory injected. That mistake was made once already; the field
    that matters is which notes were injected.

Usage: python3 bench/usage_report.py [--days N] [--json]
"""
import argparse, collections, datetime as dt, json, os, sys

LOG = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/usage.jsonl")

def load(days=None):
    if not os.path.exists(LOG):
        return []
    cutoff = None
    if days:
        cutoff = dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)
    out = []
    with open(LOG) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if cutoff:
                try:
                    when = dt.datetime.fromisoformat(rec.get("t", "").replace("Z", "+00:00"))
                    if when < cutoff:
                        continue
                except Exception:
                    pass
            out.append(rec)
    return out

def kind(rec):
    return rec.get("kind") or ""

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=None, help="limit to the last N days")
    ap.add_argument("--json", action="store_true", help="machine-readable")
    args = ap.parse_args()

    recs = load(args.days)
    if not recs:
        print(f"no records at {LOG}")
        print("The instrument landed 2026-10-14; an empty file means no memory activity since, "
              "not a missing feature.")
        return 0

    counts = collections.Counter(kind(r) for r in recs)
    refs_by_note = collections.Counter()
    for r in recs:
        if kind(r) == "session_refs":
            for ref in (r.get("refs_list") or []):
                refs_by_note[ref] += 1
    injects = [r for r in recs if kind(r) == "inject"]
    breaks = [r for r in recs if kind(r) == "prefix_break"]
    sessions_with_refs = sum(1 for r in recs if kind(r) == "session_refs")
    chars = [r.get("session_chars") or 0 for r in injects if r.get("session_chars")]

    if args.json:
        print(json.dumps({
            "records": len(recs),
            "by_kind": dict(counts),
            "sessions_with_refs": sessions_with_refs,
            "injections": len(injects),
            "prefix_breaks": len(breaks),
            "injected_chars_total": sum(r.get("chars") or 0 for r in injects),
            "sessions": len({r.get("session") for r in recs if r.get("session")}) or None,
            "top_notes": refs_by_note.most_common(10),
        }, indent=2))
        return 0

    print(f"usage report  ({len(recs)} records" + (f", last {args.days} days" if args.days else "") + ")")
    print()
    print("  by kind")
    for k, n in counts.most_common():
        print(f"    {k:<20}{n:>7}")
    print()
    print("  injection")
    print(f"    sessions that resolved refs : {sessions_with_refs}")
    print(f"    turns injected              : {len(injects)}")
    print(f"    injected characters total   : {sum(r.get('chars') or 0 for r in injects):,}")
    if chars:
        print(f"    per session (max seen)      : {max(chars):,} chars")
    print(f"    prefix breaks detected      : {len(breaks)}")
    if breaks:
        print("      a prefix break means the injected block changed mid-session, which voids every")
        print("      cache prefix after it. Investigate rather than ignore.")
    if refs_by_note:
        print()
        print("  most-injected notes")
        total = sum(refs_by_note.values())
        for note, n in refs_by_note.most_common(10):
            print(f"    {n:>4}  {100.0*n/total:5.1f}%  {note}")
        print(f"    ({total} injections over {len(refs_by_note)} distinct notes)")
    print()
    print("  not measured here, deliberately")
    print("    read-through : whether the model read an injected note is not in this record")
    print("    token cost   : characters are recorded; tokens depend on the provider's tokenizer")
    return 0

if __name__ == "__main__":
    sys.exit(main())
