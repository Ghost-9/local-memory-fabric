# Roadmap

The canonical, current roadmap is **`ROADMAP.md` in the repository**, together with the live
to-do list in issue **#14**. This page would only be a stale copy, so it does not duplicate them.

Phases, each with a checkable exit condition:

- **0.4.0 Retrieval quality** — the measured pain (Recall@3 0.30 / MRR@3 0.20). Needs the larger
  fixture (#4) before ranking changes (#3) are safe.
- **0.4.x Engineering robustness** — vendored TypeScript so the gate fails rather than skips (#5);
  confidence intervals in `bench/ab.py` (#6).
- **0.5.0 Reviewed write-back** — append-only `apply` with provenance (#11), gated on the knowledge
  base being under version control first.
- **1.0.0 Beta rollout** — install/update path, rollback story, first-use walkthrough (#9), plus
  the two `decision` items (#7, #8) that are the operator's call.

**Explicitly not planned**, with reasons: the MLX reranker (already measured and rejected), a
second ranking policy for sandboxed payloads (the sandbox reuses the existing path on purpose),
and vendoring a context-window tool (it duplicates this engine's retrieval).
