#!/usr/bin/env python3
"""#104 - near-duplicate notes, reported for review and never merged automatically.

Two near-identical notes split a term's document frequency, which biases BM25 for every query that
uses that term, and inflate the index. Neither effect announces itself.

Two signals, because they fail differently:

  content_hash equality  - byte-identical copies. Certain, and cheap.
  shingle overlap        - the same content with an edit, a reordering, or a heading change.
                           This is the case a hash cannot see and the one that actually happens.

Merging is deliberately not offered. Deciding which of two notes is canonical is an editorial
decision about the knowledge base, not a similarity score, and getting it wrong silently deletes
someone's work. This prints candidates and stops.
"""
import argparse, collections, hashlib, json, os, re, sqlite3, sys

IDX = os.path.expanduser("~/.pi/agent/scratch/memory-fabric/index.sqlite3")
SHINGLE = 8
DEFAULT_THRESHOLD = 0.5

def shingles(text):
    words = [w for w in re.split(r"[^0-9A-Za-z]+", text.casefold()) if w]
    return {tuple(words[i:i + SHINGLE]) for i in range(max(0, len(words) - SHINGLE + 1))}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--threshold", type=float, default=DEFAULT_THRESHOLD,
                    help="Jaccard overlap of 8-word shingles (default 0.5)")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--index", default=IDX)
    args = ap.parse_args()

    if not os.path.exists(args.index):
        print(f"no index at {args.index}"); return 0
    con = sqlite3.connect(args.index)
    rows = con.execute("SELECT relpath,title,body,content_hash FROM documents ORDER BY relpath").fetchall()

    by_hash = collections.defaultdict(list)
    for rel, _t, _b, h in rows:
        by_hash[h].append(rel)
    identical = {h: v for h, v in by_hash.items() if len(v) > 1}

    grams = {rel: shingles(body or "") for rel, _t, body, _h in rows}
    titles = {rel: title for rel, title, _b, _h in rows}
    paths = sorted(grams)
    clusters = []
    for i, a in enumerate(paths):
        if not grams[a]:
            continue
        for b in paths[i + 1:]:
            if not grams[b]:
                continue
            ga, gb = grams[a], grams[b]
            # Cheap size filter first: Jaccard cannot exceed |smaller| / |larger|, so skip pairs
            # whose sizes make the threshold unreachable.
            if min(len(ga), len(gb)) / max(len(ga), len(gb)) < args.threshold:
                continue
            inter = len(ga & gb)
            union = len(ga) + len(gb) - inter
            if union and inter / union >= args.threshold:
                clusters.append((round(inter / union, 3), a, b))

    if args.json:
        print(json.dumps({"documents": len(rows), "identical_groups": len(identical),
                          "similar_pairs": len(clusters),
                          "clusters": [{"similarity": s, "a": a, "b": b} for s, a, b in clusters[:20]]},
                         indent=2))
        return 0

    print(f"duplicate report over {len(rows)} indexed notes (threshold {args.threshold})")
    print()
    print(f"  byte-identical groups: {len(identical)}")
    for h, rels in identical.items():
        print(f"    {h[:12]}  {rels}")
    print()
    print(f"  near-duplicate pairs : {len(clusters)}")
    for s, a, b in sorted(clusters, reverse=True)[:20]:
        print(f"    {s:.2f}  {a}")
        print(f"          {b}")
    if not clusters and not identical:
        print("    none above the threshold")
    print()
    print("  Nothing is merged. Which note is canonical is an editorial decision, not a")
    print("  similarity score, and the wrong choice deletes work silently.")
    return 0

if __name__ == "__main__":
    sys.exit(main())
