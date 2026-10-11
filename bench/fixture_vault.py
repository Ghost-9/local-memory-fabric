"""Generate a reproducible synthetic vault plus a >=100-query gold fixture.

Contains no user or production data. Everything here is written by hand for the fixture.

Why this replaces the old 12-case fixture. `bench/recall.py` scored 1.0 recall@5 on a corpus
generated from the very terms it queries, which flatters lexical retrieval and cannot see the
failure modes the real vault actually had. This fixture is built to BREAK retrieval:

  - **camelCase identifiers** (`CliffCompaction`, `ParseNotes`) - porter does not derive parts
  - **two-letter acronyms** (`KV`, `LM`) - the old 3-char floor dropped these entirely
  - **near-universal terms** appearing in every note - must be answered and LABELLED, not silent
  - **paraphrase queries** that share no rare term with their target
  - **decoys** built from the target's own words, so a pure frequency match ranks them highly
  - **no-hit queries** across three distinct kinds (unindexed, off-topic, and pure noise)

The vault is deliberately small (24 substantive notes) so that every term is genuinely common -
the difficulty the real vault has at 117 notes.

Usage:  python3 bench/fixture_vault.py <root>   # writes the vault, prints the case count
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

# (slug, title, body) - each body is written to contain exactly the difficulty it targets.
NOTES: list[tuple[str, str, str]] = [
    ("prompt-cache", "Prompt cache economics",
     "A cache break costs far more than the token it saves: the entire prefix is re-billed at full "
     "price. Append-only context preserves the cached prefix; mutating early context invalidates "
     "everything after it."),
    ("prefix-cache-stability", "Prefix cache stability",
     "Changing the beginning of a prompt invalidates the whole conversation prefix. Measured on a "
     "430-turn session, turns whose system prompt changed missed the cache every time, against "
     "six percent when it did not."),
    ("resident-context", "Resident context is the multiplier",
     "Cache reads are cheap, so resident context size is not the enemy by itself - interruption is. "
     "But a four-hundred-thousand token prefix turns one cold start into a real cost event, so "
     "resident size is the multiplier on every invalidation."),
    ("kv-cache", "KV cache fundamentals",
     "The KV cache stores key and value attention tensors between tokens. It is why prefix reuse "
     "is possible at all, and why a broken prefix costs the full sequence again."),
    ("bm25-ranking", "BM25 lexical ranking",
     "SQLite FTS5 provides BM25 ranking over a porter-stemmed inverted index. Term frequency "
     "saturates and document frequency normalises, so a common word cannot dominate a rare one."),
    ("fts5-tokenizer", "Porter stemming in FTS5",
     "The porter tokenizer reduces inflected words to a stem. It does not split identifiers on case "
     "boundaries, so a camelCase query cannot reach the parts of a word the notes actually contain."),
    ("identifiers", "Identifier tokenization",
     "Identifiers such as CliffCompaction and ParseNotes mix case mid-token. Splitting on the case "
     "boundary lets a query for the whole name also match the part that the notes contain."),
    ("acronyms", "Two letter acronyms",
     "Common technical acronyms in this corpus are short: KV, LM, IR, QA. A minimum token length "
     "of three characters silently drops every one of them, so the exact query a person types "
     "finds nothing."),
    ("retrieval-abstention", "Abstention on weak evidence",
     "Abstention is a valid answer. When query terms appear in nearly every document they cannot "
     "discriminate, and the correct behaviour is to answer over them and label the result rather "
     "than return silence."),
    ("document-frequency", "Document frequency thresholds",
     "A term's usefulness is a property of the term, not of how many documents happen to exist "
     "today. A cutoff proportional to corpus size therefore moves whenever a note is added."),
    ("source-provenance", "Source provenance",
     "Retrieved passages cite their canonical Markdown path so a reader can verify the excerpt "
     "against the source rather than trusting a generated summary."),
    ("evidence-trail", "Evidence and lineage",
     "An evidence trail records where a claim came from, when it was observed, and whether it has "
     "been superseded. Lineage without an effective date cannot answer as-of questions."),
    ("temporal-supersession", "Temporal supersession",
     "Supersession intervals preserve historical validity: a superseded note stays reachable for "
     "as-of queries after its replacement is written, so history is visible rather than erased."),
    ("privacy-loopback", "Loopback privacy boundary",
     "A loopback endpoint means nothing leaves the machine. An unknown hostname is never treated as "
     "local to unblock a session, because that trades a silence for a leak."),
    ("provider-allowlist", "Provider allowlists",
     "An exempted provider may read canonical memory. Write access is a separate switch, off by "
     "default, because reading the vault and changing it are not the same permission."),
    ("writeback-proposal", "Write-back proposals",
     "A proposal stages a reviewable diff outside the knowledge base and is never applied "
     "automatically. Agents propose; a person approves."),
    ("append-only", "Append-only apply",
     "An apply that is append-only by construction cannot rewrite an existing line, so a bad write "
     "is recoverable by reverting the knowledge base rather than by trusting a backup."),
    ("path-symlink", "Path and symlink containment",
     "Indexing rejects aliases and symlinks that resolve outside the approved roots. Using lstat "
     "rather than stat means the link itself is refused instead of followed."),
    ("credential-filter", "Credential filters",
     "A credential-shaped value is excluded from the index and raises an alarm carrying only a "
     "fingerprint. The value is never copied anywhere, including into the alarm itself."),
    ("scope-separation", "Scope separation",
     "System, company, project and session memory are distinct scopes. Scope is resolved from the "
     "working directory where possible, and a query that cannot resolve it says so rather than "
     "silently searching everything."),
    ("incremental-index", "Incremental reindexing",
     "Skipping unchanged files makes a reindex proportional to the change rather than to the "
     "corpus, but only if the skip check is cheap and the tokenizer is part of the skip key."),
    ("determinism", "Deterministic ordering",
     "Ties in ranking must break on a stable key. Ordering that depends on an unspecified row walk "
     "is an accident of the current query plan and will not survive a plan change."),
    ("engine-parity", "Cross-engine parity",
     "Two implementations must return byte-identical payloads for the same request, including "
     "timestamps and derived values, or a divergence will be found by a user rather than a gate."),
    ("no-hit", "Absence as an answer",
     "When the index does not contain the terms, the honest response is that there is no memory on "
     "the topic. Returning a confident answer from unrelated material is worse than silence."),
]

# Filler notes reuse a universal vocabulary so common terms really are common.
# Sentinel: any filler note satisfies a universal-term query. Expanded by the scorer.
FILLER_ANY = "*filler*"

FILLER = ("Routine fixture material about catalog entries, local records, and stable metadata "
          "shared across the archive with routine notes and routine entries.")

# query -> (expected relpath or "", kind)
# kinds: rare, multi, paraphrase, common, acronym, camel, single, nohit, offdomain, noise
CASES: list[tuple[str, str, str]] = [
    # --- rare exact terms (baseline; should always work) ---
    ("CliffCompaction", "projects/identifiers.md", "camel"),
    ("parseNotes", "projects/identifiers.md", "camel"),
    ("KV cache", "projects/kv-cache.md", "acronym"),
    ("KV", "projects/kv-cache.md", "acronym"),
    ("BM25", "projects/bm25-ranking.md", "rare"),
    ("bm25", "projects/bm25-ranking.md", "rare"),
    ("FTS5", "projects/fts5-tokenizer.md", "rare"),
    ("fts5", "projects/fts5-tokenizer.md", "rare"),
    ("porter", "projects/fts5-tokenizer.md", "rare"),
    ("supersession", "projects/temporal-supersession.md", "rare"),
    ("symlink", "projects/path-symlink.md", "rare"),
    ("allowlist", "projects/provider-allowlist.md", "rare"),
    ("loopback", "projects/privacy-loopback.md", "rare"),
    ("provenance", "projects/source-provenance.md", "single"),
    ("lineage", "projects/evidence-trail.md", "rare"),
    ("determinism", "projects/determinism.md", "rare"),
    ("parity", "projects/engine-parity.md", "rare"),
    ("abstention", "projects/retrieval-abstention.md", "rare"),
    ("incremental", "projects/incremental-index.md", "rare"),
    ("append-only", "projects/append-only.md", "rare"),
    # --- multi-word, some term overlap ---
    ("prompt cache prefix", "projects/prompt-cache.md", "multi"),
    ("cache break cost", "projects/prompt-cache.md", "multi"),
    ("prefix invalidation", "projects/prefix-cache-stability.md", "multi"),
    ("resident context size", "projects/resident-context.md", "multi"),
    ("document frequency cutoff", "projects/document-frequency.md", "multi"),
    ("source provenance citation", "projects/source-provenance.md", "multi"),
    ("temporal supersession interval", "projects/temporal-supersession.md", "multi"),
    ("credential fingerprint alarm", "projects/credential-filter.md", "multi"),
    ("scope resolution working directory", "projects/scope-separation.md", "multi"),
    ("tie break ordering stable", "projects/determinism.md", "multi"),
    ("reindex unchanged files", "projects/incremental-index.md", "multi"),
    ("write-back proposal diff", "projects/writeback-proposal.md", "multi"),
    ("symlink outside approved roots", "projects/path-symlink.md", "multi"),
    ("emtpoy byte identical payloads", "projects/engine-parity.md", "multi"),
    ("ranked evidence as-of", "projects/evidence-trail.md", "multi"),
    ("token length minimum acronym", "projects/acronyms.md", "multi"),
    ("case boundary identifier", "projects/identifiers.md", "multi"),
    ("stemming inflected words", "projects/fts5-tokenizer.md", "multi"),
    ("saturation normalisation rare", "projects/bm25-ranking.md", "multi"),
    ("cold start cost event", "projects/resident-context.md", "multi"),
    # --- paraphrases: deliberately share NO rare term with the target ---
    ("why does changing the beginning cost so much", "projects/prefix-cache-stability.md", "paraphrase"),
    ("what happens when the model forgets earlier instructions", "projects/prefix-cache-stability.md", "paraphrase"),
    ("how do I keep my prompts cheap", "projects/prompt-cache.md", "paraphrase"),
    ("what makes a word too common to be useful", "projects/document-frequency.md", "paraphrase"),
    ("where did this fact come from", "projects/source-provenance.md", "paraphrase"),
    ("how do I undo something already written", "projects/append-only.md", "paraphrase"),
    ("can a program follow a shortcut outside its folder", "projects/path-symlink.md", "paraphrase"),
    ("who is allowed to read the notes", "projects/provider-allowlist.md", "paraphrase"),
    ("will this automatically change my files", "projects/writeback-proposal.md", "paraphrase"),
    ("what should I do when two answers disagree in time", "projects/temporal-supersession.md", "paraphrase"),
    ("how do I know the engine agrees with itself", "projects/engine-parity.md", "paraphrase"),
    ("why is the order of results not guaranteed", "projects/determinism.md", "paraphrase"),
    ("what is it called when a system has nothing to say", "projects/no-hit.md", "paraphrase"),
    ("how do I stop re-reading every single file", "projects/incremental-index.md", "paraphrase"),
    ("short forms for technical terms", "projects/acronyms.md", "paraphrase"),
    ("splitting a name into pieces", "projects/identifiers.md", "paraphrase"),
    ("how is a word cut down to its base", "projects/fts5-tokenizer.md", "paraphrase"),
    ("is silence better than a wrong answer", "projects/no-hit.md", "paraphrase"),
    ("how do I keep old facts visible after replacing them", "projects/temporal-supersession.md", "paraphrase"),
    ("what does it mean for a secret to be excluded", "projects/credential-filter.md", "paraphrase"),
    # --- common / near-universal terms: must ANSWER and label, never go silent ---
    # The 91 filler notes are identical apart from their entry number, so pinning one of them as
    # "the" answer would be wrong - any of them is equally correct. FILLER_ANY is the acceptable
    # set; the scorer accepts a hit in any member. This is a fixture bug, not a retrieval failure.
    ("routine", FILLER_ANY, "common"),
    ("catalog entries", FILLER_ANY, "common"),
    ("stable metadata", FILLER_ANY, "common"),
    ("local records", FILLER_ANY, "common"),
    ("shared archive", FILLER_ANY, "common"),
    ("notes", FILLER_ANY, "common"),
    ("memory", FILLER_ANY, "common"),
    ("context", FILLER_ANY, "common"),
    ("index", FILLER_ANY, "common"),
    ("query", FILLER_ANY, "common"),
    # --- no-hit, three kinds ---
    ("kubernetes ingress controller helm", "", "nohit"),
    ("postgresql vacuum autovacuum tuning", "", "offdomain"),
    ("zzzqqqxywv flibbertigibbet", "", "noise"),
    ("qwertyuiop asdfghjkl", "", "noise"),
    ("nebula orrery permafrost stratosphere", "", "nohit"),
    ("gastropod cephalopod nudibranch", "", "offdomain"),
    ("xenolith archaeometry petrography", "", "noise"),
    ("zzqq", "", "noise"),
    ("rankings are not answers", "projects/determinism.md", "paraphrase"),
    ("a tie needs a tiebreak", "projects/determinism.md", "rare"),
    ("what changes when the plan changes", "projects/determinism.md", "paraphrase"),
    ("notes that replace older notes", "projects/temporal-supersession.md", "multi"),
    ("valid from and valid to", "projects/temporal-supersession.md", "rare"),
    ("as-of query historical truth", "projects/temporal-supersession.md", "multi"),
    ("canonical markdown path citation", "projects/source-provenance.md", "rare"),
    ("verify the excerpt against the source", "projects/source-provenance.md", "paraphrase"),
    ("effective date lineage", "projects/evidence-trail.md", "multi"),
    ("superseded claim visibility", "projects/evidence-trail.md", "paraphrase"),
    ("reading is not writing", "projects/provider-allowlist.md", "paraphrase"),
    ("exempted provider read access", "projects/provider-allowlist.md", "multi"),
    ("write switch off by default", "projects/provider-allowlist.md", "multi"),
    ("unknown hostname is not local", "projects/privacy-loopback.md", "multi"),
    ("nothing leaves the machine", "projects/privacy-loopback.md", "paraphrase"),
    ("diff staged outside the base", "projects/writeback-proposal.md", "multi"),
    ("human approves the change", "projects/writeback-proposal.md", "paraphrase"),
    ("revert the knowledge base", "projects/append-only.md", "paraphrase"),
    ("cannot rewrite an existing line", "projects/append-only.md", "multi"),
    ("applying twice is idempotent", "projects/append-only.md", "rare"),
    ("lstat rather than stat", "projects/path-symlink.md", "rare"),
    ("resolve outside approved roots", "projects/path-symlink.md", "multi"),
    ("fingerprint not the value", "projects/credential-filter.md", "multi"),
    ("excluded from the index", "projects/credential-filter.md", "multi"),
    ("when should the engine stay quiet", "projects/retrieval-abstention.md", "paraphrase"),
    ("weak evidence decline", "projects/retrieval-abstention.md", "multi"),
    ("routine notes routine entries", FILLER_ANY, "common"),
    ("archive material", FILLER_ANY, "common"),
    ("catalog", FILLER_ANY, "common"),
    ("stable", FILLER_ANY, "common"),
    ("how do I get it to say it has nothing", "projects/no-hit.md", "paraphrase"),
    ("confident answer from unrelated material", "projects/no-hit.md", "paraphrase"),
    ("skip files that did not change", "projects/incremental-index.md", "paraphrase"),
    ("tokenizer part of the skip key", "projects/incremental-index.md", "multi"),
    ("two implementations byte identical", "projects/engine-parity.md", "multi"),
    ("timestamp format agreement", "projects/engine-parity.md", "rare"),
    ("found by a user not a gate", "projects/engine-parity.md", "paraphrase"),
    ("BM25 porter unicode61 tokenizer", "projects/bm25-ranking.md", "multi"),
    ("inverted index stem", "projects/fts5-tokenizer.md", "multi"),
    ("case boundary mid-token", "projects/identifiers.md", "multi"),
    ("LM IR QA short forms", "projects/acronyms.md", "multi"),
    ("minimum token length two characters", "projects/acronyms.md", "multi"),
    ("one letter fragments excluded", "projects/acronyms.md", "rare"),
    ("prefix re-billed at full price", "projects/prompt-cache.md", "multi"),
    ("append only preserves reuse", "projects/prompt-cache.md", "paraphrase"),
    ("six percent miss rate", "projects/prefix-cache-stability.md", "rare"),
    ("430-turn session measurement", "projects/prefix-cache-stability.md", "multi"),
    ("system prompt changed", "projects/prefix-cache-stability.md", "rare"),
    ("multiplication on every invalidation", "projects/resident-context.md", "paraphrase"),
    ("400K prefix cold start", "projects/resident-context.md", "multi"),
    ("cutoff moves when notes added", "projects/document-frequency.md", "paraphrase"),
    ("absolute floor generous ceiling", "projects/document-frequency.md", "rare"),
    ("noise query alpha beta gamma", "", "noise"),
    ("zeta eta theta iota kappa", "", "nohit"),
    ("orchard vineyard silo granary", "", "offdomain"),
    ("mongoose weasel badger otter", "", "noise"),
    ("html css javascript python rust", "", "nohit"),
    ("lorem ipsum dolor sit amet", "", "noise"),
]


def create(root: Path) -> list[dict[str, object]]:
    projects = root / "projects"
    projects.mkdir(parents=True, exist_ok=True)
    for slug, title, body in NOTES:
        (projects / f"{slug}.md").write_text(f"# {title}\n\n{body}\n", encoding="utf-8")
    # Decoys reuse each target's own vocabulary so frequency alone ranks them competitively.
    for slug, _title, body in NOTES:
        words = body.split()
        decoy = " ".join(words[:8] + words[:8])
        (projects / f"decoy-{slug}.md").write_text(f"# Decoy {slug}\n\n{decoy}\n", encoding="utf-8")
    filler = projects / "filler"
    filler.mkdir(parents=True, exist_ok=True)
    for i in range(91):
        (filler / f"note-{i:03}.md").write_text(
            f"# Synthetic filler note {i:03}\n\n{FILLER} Entry {i:03}.\n", encoding="utf-8")
    return [{"query": q, "expected": [e] if e else [], "kind": k} for q, e, k in CASES]


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    root = Path(sys.argv[1])
    cases = create(root)
    out = root / "fixture-cases.json"
    out.write_text(json.dumps(cases, indent=1), encoding="utf-8")
    kinds: dict[str, int] = {}
    for c in cases:
        kinds[c["kind"]] = kinds.get(c["kind"], 0) + 1
    print(f"notes={len(NOTES)} decoys={len(NOTES)} filler=91 cases={len(cases)}")
    print("kinds: " + ", ".join(f"{k}={v}" for k, v in sorted(kinds.items())))
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())