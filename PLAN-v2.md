# Engram v2 — the plan

**Written 2026-10-20.** Three phases, **127 numbered items**. Every item below is a real defect, gap or
measured weakness in this repository — not a wish. Each carries the fact that justifies it and the fix
that closes it. Items are ordered so that nothing depends on something later in the list.

**Scale:** Phase 1 = **45 items** (one minor release, `0.5.0`). Phase 2 = **42 items** (`0.6.0`).
Phase 3 = **40 items** (`1.0.0`).

**Why this plan exists.** Today's audit found the gate red on a correct migration, an index 66% larger
than it needs to be, a retrieval engine that finds the right note in the top three 93% of the time but
first only 47%, 131 issues of which 128 are closed and 3 open, and a feature — prompt injection — with
168 recorded injections and 6 reads. The engineering is sound. The product is not yet a product.

---

## Phase 1 — Small, fast, honest (45 items, `0.5.0`)

The theme is **subtraction**. Every item below removes something: bytes, a race, a claim that is not
true, or a step a user has to know about.

### 1A. Injection on hold (items 1–6)

Measured facts: `inject` 168 records, `read` 6. A 72-run task suite shows a median per-pair delta of
**+0.0%** on the vault tasks it was designed for. `index.ts:914` returns `{ message: … }` from
`before_agent_start`, which inserts a message and therefore mutates the prompt. `injectedThisSession`
is latched on the first turn with refs and **has no turn guard**, so it can fire at turn 30 and
invalidate every cached prefix before it.

1. **Hold injection by default.** Flip the default so injection is off unless explicitly enabled;
   today it is on unless `MEMORY_FABRIC_DISABLE_INJECTION=1`. Reason: unproven benefit, non-zero cost.
2. **Add the missing turn guard.** Even when enabled, injection may fire only on turn 0/1, before any
   prefix is cached. A mid-session injection is a cache invalidation with no measured benefit.
3. **Keep the capability, change the direction: pull, not push.** Refs arrive through an explicit tool
   call (`memory_fabric_context`) that the model chooses to make. This is the "visible-by-default"
   decision already taken for the six promoted tools, applied consistently.
4. **Record the hold in the ledger** as `injection_held` with the reason, so the decision is auditable
   and a future reader can see it was deliberate rather than forgotten.
5. **Remove the injection claims from README/CHANGELOG** that imply measured benefit. Replace with the
   actual numbers: 168 injections, 6 reads, +0.0% median.
6. **Add a gate assertion that the default is held**, so an unrelated refactor cannot silently turn it
   back on. Reason: this project's documented failure mode is a feature being re-enabled by drift.

### 1B. Index size — 66% of it is dead weight (items 7–19)

Measured: 20.1 MB / 130 documents = **162,359 bytes per document**. `doc_terms` 5.64 MB,
`doc_terms_by_term` 5.56 MB, `documents_fts_content` 1.98 MB. The first two were the sole source of
document frequency until commit `556a0fb`; df now comes from `fts5vocab`. The third is a second copy
of every body.

7. **Drop `doc_terms`.** Reachable only through `matched_terms` scoring, which is being replaced (8).
8. **Replace `matched_terms` with the FTS5 match itself** (`fts5vocab`/`highlight`), so scoring reads
   what FTS actually matched rather than a parallel table that can disagree — which is exactly the bug
   `69af6c8` had to fix.
9. **Drop `doc_terms_by_term`** with its parent table.
10. **Convert `documents_fts` to an external-content table**, removing `documents_fts_content`
    (1.98 MB) and making the note body exist once instead of twice.
11. **Migration 3 → 4**, expand-only, recorded, reconciled — the existing ladder in `paths.rs`.
12. **`propose.rs:222`** rewrite: stop rebuilding `doc_terms`.
13. **`index.rs` vacuum**: its orphan check reads `doc_terms`; retarget to `documents`.
14. **`db.rs` reconciliation**: its derived-table assertion names `doc_terms`; retarget.
15. **`memory_fabric.py`** port of 7–14, so parity holds. The parity gate makes this atomic.
16. **`bench/golden/format.json`** update: new table set, new byte ceilings.
17. **Format gate asserts `bytes_per_document` below a ceiling**, not just the table list. Today it
    cannot catch a 20% size regression — it only catches a new table.
18. **Re-measure and publish the result**, including if it is worse than the ~56 KB estimate.
19. **Update `growth_warn_at_documents`** to reflect the new footprint; it currently fires at 3,306.

Target: **below 60 KB per document**, from 162 KB. This is the item that makes a large vault viable and
it is mechanical — deletion plus four call-site rewrites, all covered by the parity contract.

### 1C. Ordering — the 46-point gap (items 20–29)

Measured: recall@3 **0.9344**, recall@5 0.9426, recall@1 **0.4672**. The target is in the top three
93% of the time and first 47% of the time. The gap from @3 to @5 is under one point, so the residual
misses are not rank-4/5 problems. **Retrieval is nearly solved; ordering is not.**

20. **Make MRR the primary objective, not recall.** The SLO gate lists `recall_at_3` and `mrr_at_3`;
    the work is currently optimised against the one that is already at 93%.
21. **Add `recall_at_1` as an SLO objective.** A metric that is not gated is not defended.
22. **Retune `SECTION_WEIGHT`** (currently 0.15) with a paired A/B, not a single run.
23. **Retune the self-repetition penalty** (`SELF_REPETITION_WEIGHT`, 1.0) the same way.
24. **Retune title and heading priors**, currently hand-set.
25. **Use the link graph.** `note_links` is populated and **not consulted by ranking**. A note that
    other notes reference is more likely to be the answer.
26. **Two-stage rank:** wide candidate set by BM25, then rescore the top ~20 with exact section match.
    Today one scoring pass does both jobs poorly.
27. **Add ordering-specific golden cases.** The current fixture's 6 decoys per note test retrieval;
    they do not isolate "right note, wrong rank".
28. **Publish the per-weight A/B results**, including losses. This project has a documented habit of
    reporting only the winning configuration.
29. **Set an explicit ordering target** — recall@1 ≥ 0.60 before Phase 3 begins, or the phase is not
    complete.

### 1D. Reliability — the gate must never lie (items 30–37)

Measured: today the gate went **red on a correct migration** because the test raced the live vault.

30. **Make every gate test hermetic**, not just the migration test. Anything that asserts "nothing
    changed" against the live vault is racy by construction.
31. **Add a gate assertion that no test reads the live vault path.** Source-level check is weak; a
    runtime check on the resolved index path is not.
32. **Fail loudly on an empty corpus** in every test, not silently skip. An empty index makes all
    comparisons trivially equal — how a broken test reports success.
33. **Resolve the one unresolved secret alarm** (`pi-harness/30-t3-pi-driver.md`, since 2026-10-13,
    drain rate 0%). Either scrub it or record an accepted-risk decision with a date.
34. **Gate: zero uncommitted vault files.** Today there were 11, which means no revert point — the
    exact condition D4 exists to prevent.
35. **Gate: `search` must be blocked without a config allowlist.** Verified manually today; asserting
    it prevents a future default from opening the vault.
36. **Add a flake detector**: run the two timing-sensitive steps twice and fail on disagreement.
37. **Gate summary line states what was measured**, not just "OK" — a green light with no numbers is
    unreadable in six months.

### 1E. Usable by someone who is not me (items 38–45)

Today a new user must know: where the vault lives, that `index` must be run, that a config file
governs the allowlist, and that a fresh state directory is fail-closed on search with a message that
does not say how to fix it.

38. **`engram init`** — one command: create config, detect an existing vault (Obsidian or Markdown),
    index it, print what it found.
39. **`engram doctor`** — diagnose the actual state: index present, integrity, config allowlist, vault
    recoverability, backup freshness, gate status. One command, no arguments.
40. **Config schema with validation.** A malformed config currently fails closed at *search* time with
    a security message; it should fail at *load* time with a field-level error.
41. **A default config that works without editing.** Zero-edit install is the bar.
42. **Actionable error messages.** "Memory content is available only to an explicitly allowed local
    model provider" is correct and unhelpful. It must name the file and the field.
43. **`--help` and `--version` that list the real operations.** The engine implements 30 ops; the README
    documents 7.
44. **Quickstart under 60 seconds**, tested on a clean machine, timed.
45. **A real uninstall path** — where state lives, what to delete, what is left behind.

**Phase 1 done means:** injection held and documented, index below 60 KB/doc, recall@1 ≥ 0.60, every
gate test hermetic, `engram init` working from zero, and a README whose every claim is measured.

---

## Phase 2 — Scopes, learning, governance (42 items, `0.6.0`)

The theme is **the memory becomes a memory**: it is scoped, it learns from mistakes, and it cannot be
silently corrupted.

### 2A. Scopes — global, per-agent, cross-agent, section (items 46–57)

Today the tier rule is binary: `projects/` is project-scoped, everything else is global. There is no
per-agent memory, no cross-agent sharing, and no way to retrieve one section deliberately.

46. **Define the scope model explicitly**: `global`, `project:<slug>`, `agent:<name>`,
    `project:<slug>/agent:<name>`, and `section:<relpath>#<anchor>`.
47. **Write the classification table into config**, not into code, so it is inspectable and editable.
48. **Per-agent memory directory** (`agents/<name>/`) with the same append-only contract as the vault.
49. **Cross-agent promotion**: a note earns `global` by being referenced from ≥ N distinct projects.
    Measured, not declared — the data to do this already exists in `note_links`.
50. **Section-scoped retrieval as a first-class scope**, not a post-filter. `[[ref#anchor]]` already
    exists; the scope does not.
51. **Scope-aware ranking**: a project-scoped note outranks a global one for the same query inside
    that project, without hiding the global one.
52. **`engram scope list|show|diff`** so a user can see what an agent can see.
53. **Fail-closed on unknown scopes**, matching the existing allowlist discipline.
54. **Retrieval tests per scope**, including the leak case: agent A must not retrieve agent B's notes.
55. **Cross-agent test**: two agents, one shared note, assert both see it and neither sees the other's
    private note.
56. **Document the model** with a diagram, not prose.
57. **Migrate existing `projects/` notes** into the new model without moving files on disk.

### 2B. Recursive learning — from mistakes (items 58–69)

Today "learning" means an agent writes a note. There is no capture of *what went wrong*, no promotion
of a repeated correction, and no way to see what the system learned.

58. **A correction is a first-class record**: what was believed, what is true, the evidence, the date.
    This project has retracted twelve claims in one session; none of those retractions is retrievable
    as a lesson.
59. **Failure capture on tool error**: a failed command shape plus its fix is exactly the kind of
    durable fact the vault should hold. `invariants/tool-quirks.md` does this by hand.
60. **Promotion rule**: a correction seen twice becomes an invariant; seen once it stays an anecdote.
    Deterministic, inspectable, no model in the loop.
61. **Demotion rule**: a fact contradicted by newer evidence is marked, not deleted — the vault
    contract is append-not-rewrite.
62. **`engram learn` review** — list what was captured this week that has not been read.
63. **Learned facts are retrievable by the same search path**, so learning and memory are one system.
64. **A measured learning signal**: does the vault contain the answer to a question it previously got
    wrong? Testable with the existing task suite.
65. **Never learn silently.** A proposed write is proposed, approved, then written — the existing
    `propose`/`approve` ops, wired to the learning path.
66. **Provenance on every learned fact**: which session, which evidence.
67. **A refusal path**: if the evidence is a model's assertion and not a measurement, it is recorded as
    unverified. This is the `WEAK`/`STRONG` distinction, enforced at write time.
68. **Learning tests**: a synthetic "got it wrong, then right" sequence, asserting the second attempt
    retrieves the correction.
69. **Publish the measurement** or report that the signal did not appear.

### 2C. Write governance (items 70–79)

ROADMAP already names this: propose/approve, no silent overwrite, provenance validation. It is
unbuilt.

70. **No silent overwrite, enforced** in the write path, not only in the proposal path.
71. **Provenance required** for any write that changes an existing claim.
72. **Review queue with a drain rate**, and an alert when the drain rate is 0% — today 13 proposals
    were never applied and nothing said so.
73. **Conflict detection**: two agents writing the same note in one window.
74. **Append-only enforcement** with an explicit exception list, tested.
75. **Write budget** already exists (50); surface it in `doctor`.
76. **Audit log per note**: who wrote what, when, from which session.
77. **Revert a single write** without reverting the commit — the pre-write `git add` makes this
    possible; there is no command for it.
78. **Governance tests**: an overwrite attempt must fail closed.
79. **Document the contract** in one page a new user can read.

### 2D. Importer and the oracle (items 80–87)

80. **`engram import`** for a Markdown/Obsidian vault: frontmatter, links, attachments, folder rules.
81. **Import is opt-in with a recorded approver** — the operator-only flag, unchanged.
82. **Dry-run by default**, diff before writing.
83. **Port the 28 security-contract tests to Rust BEFORE retiring the Python oracle.** This ordering is
    not negotiable: the oracle is the reference implementation and retiring it first removes the check.
84. **Then retire the oracle**, removing 1,972 of 11,577 lines (17% of the codebase).
85. **Property tests** comparing Rust against itself across the retired surface, so retirement does not
    reduce coverage.
86. **`main.rs` split** (1,186 lines) into one module per operation group.
87. **Coverage report in the gate**, so "retired with confidence" is a number.

---

## Phase 3 — Exhibition product (40 items, `1.0.0`)

The theme is **it has to be obvious in ninety seconds**, on a screen, to someone who has never heard of
it.

### 3A. Performance with a published method (items 88–97)

88. **A repeatable benchmark harness** that reports median, CI and n — the standing rule, mechanised.
89. **Warm/cold split** in every published number.
90. **A scale curve** to 10,000 documents, measured, not extrapolated.
91. **Query latency percentiles** p50/p90/p99, on the real vault.
92. **Index build throughput** documents/second at three corpus sizes.
93. **Memory footprint** (RSS) at idle and under search.
94. **Cold-start time** from binary invocation to first result.
95. **A competitor comparison on the same corpus**, with the method published, including where we lose.
96. **Regression thresholds in CI** that fail on a 10% degradation.
97. **A public results page** generated from the harness output, not hand-written.

### 3B. The ninety-second demo (items 98–107)

98. **A single-command demo** that creates a vault, indexes it, asks three questions, and shows the
    retrieved notes with timings.
99. **A terminal recording** of that command, committed, so it cannot drift from reality.
100. **A visible answer to "how much space"** — bytes per document on screen.
101. **A visible answer to "how fast"** — p50 on screen.
102. **A cross-session demonstration**: run a task, hand off, resume in a fresh session for ~300
     tokens. This is the product's strongest measured claim (1,790,337 tokens avoided) and it is
     invisible today.
103. **A learning demonstration**: get it wrong, correct it, retrieve the correction.
104. **Offline proof**: the demo runs with networking off.
105. **A one-page comparison sheet** against the obvious alternatives, factual, losses included.
106. **A slide that says what it is not** — not an embeddings product, not a cloud service, not a
     multi-user server.
107. **The demo must pass in the gate**, so it cannot rot.

### 3C. Distribution and trust (items 108–118)

108. **Installable without a checkout** — a package or a signed binary.
109. **Version pinning and a changelog a user can follow.**
110. **Upgrade path with migrations tested from every prior version.**
111. **Data portability**: export everything to plain Markdown, tested, one command.
112. **A documented backup story** including the off-machine bundle already built.
113. **A security page** stating the allowlist model, the fail-closed defaults, and what leaves the
     machine (nothing, by default).
114. **A licence page** — AGPL-3.0, what it means for a user, in plain language.
115. **A contribution path** with the gate as the contract.
116. **An issue template that asks for the measurement**, because this project's history is a series of
     claims that needed one.
117. **A reproducibility page**: every published number, and the command that regenerates it.
118. **A support boundary**: what is covered, what is not, stated plainly.

### 3D. The claims must survive scrutiny (items 119–127)

119. **Every published number carries n, method and date.** No exceptions.
120. **Losses published as prominently as wins** — the 168-injection/6-read result belongs on the front
     page, not in a footnote.
121. **Retractions are part of the record**: twelve claims were retracted in one session; the product
     page should link to that history rather than present only the current state.
122. **An external review before launch**, on the model of the two rounds that falsified my own claims
     twice this month. Budget for it being right.
123. **A red-team pass on the confidentiality claims.**
124. **A cold-start user test**: someone who has never seen it installs it from the README alone, and
     every point of confusion is a bug.
125. **A performance claim audit**: every "faster" claim is paired against something.
126. **A final honesty pass**: delete any claim that cannot be reproduced by the published command.
127. **Tag `1.0.0` only when 1–126 hold**, not on a date.

---

## What is deliberately NOT in this plan

- **Embeddings.** They break the size story (the entire index is under 60 KB/doc after Phase 1; a
  vector per section is larger than the note), they add a model dependency, and they are unmeasured
  here. `context-mode` and `billion-context` are rejected for related reasons, on the record.
- **A MITM proxy** for context capture. It is a trust-boundary change and the operator has not
  approved it.
- **Multi-user or hosted serving.** The confidentiality model is a local allowlist; a server changes
  what the product is.
- **Competing with Mem0/Zep/Letta/cognee on retrieval quality.** The differentiator is durable,
  confidential, local memory with a receipt.
- **Any feature whose value has not been measured**, including the one being held in item 1.

## How progress will be reported

Every phase closes with the same four artefacts, and no phase is complete without them:

1. **The gate, green, with its numbers**, not just a pass line.
2. **A measured before/after** for each item that claims an improvement, paired, with n.
3. **A loss list** — what got worse, stated at the same volume.
4. **A reproducible command** for every number, checked into the repo.

## First three things, starting now

1. **Item 1**: hold injection by default and add the turn guard. It is a two-line change with a
   measured basis and it removes a live cache risk.
2. **Items 7–10**: drop the two term tables and convert FTS to external content. Largest single win
   (66% of the index), fully covered by the parity contract.
3. **Items 20–21**: make MRR and recall@1 the gated objectives, so the ordering work has a target
   instead of an opinion.
