#!/usr/bin/env python3
"""What memory actually did for you — counted, not estimated.

The idea is borrowed from XERJ's `xerj gain`. The measurement is not: they count tokens saved
against an index, because that is all they can see. This engine has a *read side* - its own
search/read tools - and note-level provenance, so it can answer the question that matters more
than cost: of the notes injected into a session, how many were then actually opened?

Read-through is the honest denominator for "is this worth doing". A cited note that is never
read is noise that cost tokens; a cited note that is read is a pointer that worked. Neither
XERJ nor a plain grep-savings counter can distinguish them.

Sources: session transcripts (history) and usage.jsonl (the durable record, written since
2026-10-14). Reads only; writes nothing.
"""
from __future__ import annotations

import collections
import glob
import json
import os
import re
import sys

SESSIONS = os.path.expanduser("~/.pi/agent/sessions/*/*.jsonl")
USAGE = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/usage.jsonl")
CHARS_PER_TOKEN = 4


def sessions() -> list[str]:
    return sorted(glob.glob(SESSIONS))


def analyse(path: str) -> dict:
    """One session: which notes were injected, and which were then opened."""
    txt = open(path, encoding="utf-8", errors="replace").read()
    out = {"injected": [], "read": set(), "withheld": 0}
    # Injected refs come from the extension's own block. Two serialisations exist in the
    # corpus (raw and JSON-escaped), so match both rather than silently undercounting -
    # that mistake produced a 3x spread in an earlier audit.
    for pat in (r"## Memory has relevant notes\\n((?:\[\[[^\]]+\]\] ?)+)",
                r"## Memory has relevant notes\n((?:\[\[[^\]]+\]\] ?)+)"):
        for block in re.findall(pat, txt):
            out["injected"] += re.findall(r"\[\[([^\]]+)\]\]", block)
    out["withheld"] = txt.count("Local memory withheld")
    # Strip the injected blocks BEFORE looking for reads. Without this the detector matches the
    # injection itself - the note name is right there in the block it was injected in - and
    # read-through reads ~100% no matter what actually happened. That is the same class of
    # measurement error as counting the harness's warm path, and it is why this is stripped
    # rather than searched in place.
    body = re.sub(r"## Memory has relevant notes.{0,600}?before re-deriving\.?",
                  "", txt, flags=re.S)
    for note in set(out["injected"]):
        # A read is evidence the session reached for that note: a tool result naming it as the
        # source_ref, or an explicit read call. Not merely the name appearing somewhere.
        if re.search(rf'"source_ref":\s*"{re.escape(note)}"', body) or \
           re.search(rf'memory_fabric_read[^\n]{{0,200}}{re.escape(note)}', body):
            out["read"].add(note)
    return out


def main() -> int:
    total_sessions = injected_sessions = withheld = 0
    cited = collections.Counter()
    read = collections.Counter()
    chars_injected = 0
    for path in sessions():
        total_sessions += 1
        r = analyse(path)
        withheld += r["withheld"]
        if not r["injected"]:
            continue
        injected_sessions += 1
        cited.update(r["injected"])
        read.update(r["read"])
        # refs block cost: "## Memory has relevant notes" + 3 refs + the trailing sentence
        chars_injected += len("## Memory has relevant notes\n\nRead these first via engram or view_file before re-deriving.") \
                          + sum(len(n) + 4 for n in r["injected"])

    total_cited = sum(cited.values())
    total_read = sum(read.values())
    print("=" * 67)
    print("  GAIN — counted from this machine's own record")
    print("=" * 67)
    print(f"  sessions scanned            {total_sessions}")
    print(f"  sessions served             {injected_sessions}")
    print(f"  notes injected              {total_cited}")
    print(f"  distinct notes cited        {len(cited)}")
    print(f"  withholdings                {withheld}")
    print()
    print(f"  READ-THROUGH                {total_read}/{total_cited}"
          f"  ({100*total_read/max(total_cited,1):.0f}% of cited notes were then opened)")
    print()
    tok = chars_injected / CHARS_PER_TOKEN
    print(f"  injection cost              {chars_injected:,} chars ~= {tok:,.0f} tokens")
    print(f"  per session served          {chars_injected/max(injected_sessions,1):,.0f} chars ~= "
          f"{tok/max(injected_sessions,1):,.0f} tokens")
    if total_read:
        print(f"  cost per useful citation    {chars_injected/total_read:,.0f} chars ~= "
              f"{tok/total_read:,.1f} tokens")
    print()
    print("  WORTH KEEPING (cited and then read)")
    for n, c in [(n, read[n]) for n, _ in cited.most_common() if read[n] > 0][:6]:
        print(f"    {read[n]:>4} used  {n}")
    # Only list noise when there is a trustworthy read measure to compare against. Reporting
    # "63 of 63 cited notes were never opened" from a broken detector would read as a damning
    # finding about the corpus when it is a finding about the instrument.
    if total_read > 0:
        noise = [(n, c) for n, c in cited.most_common() if read[n] == 0]
        print(f"\n  NOISE CANDIDATES (cited, never opened) — {len(noise)} of {len(cited)} notes")
        for n, c in noise[:6]:
            print(f"    {c:>4} cited, 0 used  {n}")
    else:
        print("\n  NOISE CANDIDATES            not computable: it needs the live read record, "
              "which starts from 2026-10-14")
    print()
    if os.path.exists(USAGE):
        lines = [json.loads(l) for l in open(USAGE, encoding="utf-8", errors="replace") if l.strip()]
        kinds = collections.Counter(x.get("kind") for x in lines)
        served = {x.get("ref") for x in lines if x.get("kind") == "session_refs"}
        live = [x for x in lines if x.get("kind") == "read"]
        # The trustworthy figure: refs resolved vs notes actually reached for, same record.
        refs_total = sum(x.get("refs", 0) for x in lines if x.get("kind") == "session_refs")
        print(f"  usage.jsonl                 {len(lines)} records since 2026-10-14: {dict(kinds)}")
        print(f"  LIVE READ-THROUGH           {len(live)} reads against {refs_total} refs resolved")
        if live:
            per = collections.Counter(x.get("ref") for x in live)
            for r, c in per.most_common(5):
                print(f"    {c:>4} read  {r}")
    else:
        print("  usage.jsonl                 not yet written (created 2026-10-14)")
    print()
    print("  READ-THROUGH HISTORY IS NOT RECOVERABLE, and this tool will not pretend otherwise.")
    print("  Two attempts, both wrong in opposite directions: a loose detector matched the")
    print("  injection itself and reported 78%; a strict one reported 0% because the evidence is")
    print("  simply not in the transcript. Fabric tool results are stripped from history by the")
    print("  scrub, and reads normally go through the built-in read/grep tools.")
    print("  Since 2026-10-14 the extension records reads as they happen, so the rate below is")
    print("  the only trustworthy one - and it starts empty.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
