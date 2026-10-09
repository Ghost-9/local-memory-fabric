"""Generate a reproducible synthetic vault; contains no user or production data."""
from __future__ import annotations

from pathlib import Path

CASES = [
    {"query": "temporal supersession interval", "expected": ["projects/bench-01.md"]},
    {"query": "source provenance citation lineage", "expected": ["projects/bench-02.md"]},
    {"query": "privacy provider loopback allowlist", "expected": ["projects/bench-03.md"]},
    {"query": "writeback proposal approval diff", "expected": ["projects/bench-04.md"]},
    {"query": "index symlink traversal containment", "expected": ["projects/bench-05.md"]},
    {"query": "retrieval abstention rareterm threshold", "expected": ["projects/bench-06.md"]},
    {"query": "scope company project episodic", "expected": ["projects/bench-07.md"]},
    {"query": "sqlite fts5 porter ranking", "expected": ["projects/bench-08.md"]},
    {"query": "cache append context prefix", "expected": ["projects/bench-09.md"]},
    {"query": "proposal secret credential heuristic", "expected": ["projects/bench-10.md"]},
    {"query": "nebula orrery permafrost", "expected": []},
    {"query": "quasar glacier astrolabe", "expected": []},
]

TARGETS = [
    ("Temporal policy", "Temporal supersession intervals preserve historical source validity and explicit as-of retrieval."),
    ("Evidence trail", "Source provenance citation lineage ties retrieved passages to their canonical Markdown."),
    ("Provider boundary", "Privacy provider loopback allowlists ensure excerpts stay with configured local inference."),
    ("Review workflow", "Writeback proposal approval diff creates review-only candidates without applying changes."),
    ("Path safety", "Index symlink traversal containment rejects aliases and paths outside approved roots."),
    ("Search abstention", "Retrieval abstention rareterm threshold declines weak matches instead of fabricating evidence."),
    ("Scope separation", "Scope company project episodic distinguishes shared, project, and session memory."),
    ("Lexical ranking", "SQLite FTS5 Porter ranking supplies the local lexical candidate order."),
    ("Prompt cache", "Cache append context prefix preserves stable prompt prefixes across turns."),
    ("Secret review", "Proposal secret credential heuristic rejects suspicious values before staging."),
]


def create_synthetic_vault(root: Path) -> list[dict[str, object]]:
    """Create 111 indexed synthetic Markdown files plus one filtered candidate."""
    projects = root / "projects"
    projects.mkdir(parents=True, exist_ok=True)
    for index, (title, body) in enumerate(TARGETS, start=1):
        (projects / f"bench-{index:02}.md").write_text(f"# {title}\n\n{body}\n", encoding="utf-8")
        # A deliberately incomplete decoy tests ranking against partial term overlap.
        words = body.split()
        decoy_body = " ".join(words[:6] + words[:6] + words[:6])
        (projects / f"decoy-{index:02}.md").write_text(f"# Decoy {index:02}\n\n{decoy_body}\n", encoding="utf-8")
    for index in range(91):
        path = projects / "filler" / f"note-{index:03}.md"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            f"# Synthetic filler note {index:03}\n\n"
            f"Routine fixture material about catalog entry {index:03}, local records, and stable metadata.\n",
            encoding="utf-8",
        )
    # Compose a credential-shaped value at runtime so no token-like literal is shipped.
    generated_candidate = "SUPABASE_SERVICE_ROLE_KEY=" + ("CANDIDATE" + "X" * 24)
    (projects / "filtered-candidate.md").write_text(f"# Filter test\n\n{generated_candidate}\n", encoding="utf-8")
    return CASES
