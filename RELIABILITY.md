# Reliability: what is protected, what is disposable, and what is not yet true

Written 2026-10-15. Every figure is measured on this machine by `bench/baseline.py` or by
`status`. **Read the last section — it lists what is still missing.**

## RPO / RTO, stated (D1)

The system has exactly **one** irreplaceable component: the Markdown knowledge base. Everything
else can be rebuilt, so the targets differ per component and stating them once settles every
later decision.

| component | size | RPO | RTO | why |
|---|---|---|---|---|
| **the vault** | 34 MB, 205 tracked files | **0 changes** — every committed change is kept | measured by the restore drill | the only thing a person wrote |
| the index | 13.6 MB | **unlimited** — losing it costs nothing | **~42 ms per 200 notes** | derived from the vault, in a measured time |
| the engine binary | 4.6 MB | unlimited | 39 s (`cargo build --release`) | built from source in the repo |
| `usage.jsonl` | small | one session | — | a behaviour record, not memory |

**The consequence is the whole design:** there is no reason to back up the index, and no excuse
for not backing up the vault. Spending effort on index durability is spending it on the
reproducible half.

## Status reports recoverability (O3)

`engram status` answers "can I recover?" rather than leaving it inferred:

```
vault_history: {
  under_version_control: true,
  head: "6803b7b1",
  head_committed_at: "2026-10-15T01:08:07+05:30",
  uncommitted_files: 9,
  fully_recoverable: false        <- the honest answer right now
}
```

`fully_recoverable` is false whenever anything is uncommitted, because a file with no committed
baseline cannot be reverted — `git restore` would discard the work it is protecting.

## Alarm queue (O2)

`status` reports `unresolved_alarms` and `oldest_unresolved_alarm`. **Current state: 1 unresolved,
open since 2026-10-13 — a 0% drain rate.** The engine detects credential-shaped content in
indexed notes and records an alarm; nothing was resolving them. A queue nobody empties is a queue
nobody reads during an incident, which is worse than no queue.

## Growth (P1)

Measured **118,039 bytes per document**. `status` now reports `bytes_per_document`,
`growth_warn_at_documents` (where the index reaches 512 MB — currently 4,548 documents) and a
`growth_warning` flag. Capacity is planned rather than discovered.

## Latency (A3, O4)

- The SQLite busy timeout is now a **jittered bounded retry** rather than one flat 5 s wait, so two
  colliding writers back off to different instants instead of retrying in lockstep.
- Per-operation latency is kept as a **rolling in-memory histogram** and flushed with the session
  record — no I/O on the hot path, and `p50`/`p95` per operation is available per session.

## Write budget (A4)

Remote write-back is on by default, so the extension counts canonical-memory writes per session
and refuses past **50**, logging every write. The refusal message says plainly that it is a guard
against a loop, not a policy, and that restarting the session clears it.

## Off-machine backup (D2) and the first restore drill (D3)

Destination, auto-selected by the operator's instruction and changeable with one variable:
`~/Library/Mobile Documents/com~apple~CloudDocs/engram-vault-backup/` (iCloud Drive, 59 GB free).
`ENGRAM_BACKUP_DIR` overrides it.

**A git bundle, not a synced repository.** Putting a live `.git` directory inside a file-sync
service invites corruption: git writes objects incrementally and a sync client can copy a
half-written pack. A bundle is one immutable file — a sync client copies it whole or not at all.
Measured: **8.3 MB** for the full history.

**One-directional, and the vault stays un-synced.** No service is given live access to the
canonical store and no client can write to it. Restoring is a deliberate act
(`scripts/vault-backup.sh --verify` clones a bundle into a temp directory and compares tree
hashes). The contract's *no sync, no publish* holds: what is backed up is an **artefact derived
from** the vault, not the vault itself.

**Rotation keeps it small, as asked:** 7 bundles by default, hard cap 500 MB, oldest evicted first
by either rule.

**First restore drill, measured:**

```
bundle      : vault-2026-10-15.bundle
RTO         : 0s (a single-file clone)
files       : 210 restored vs 210 live
tree hash   : 55e068a5…   matches live: YES
sample note : byte-identical
```

Before this, the honest description was *correct, fast, and one disk failure away from total
loss*. That sentence no longer applies.

## All 23 milestones — what landed, and what the measurement changed

Every row was implemented and tested. Three of them changed conclusion on contact with a
measurement, and those are the interesting ones.

### Durability

| # | what | evidence |
|---|---|---|
| D1 | RPO/RTO stated per component | this file; the index is disposable, the vault is not |
| D2 | Second copy of the vault | iCloud Drive bundle, 8.3 MB; `scripts/vault-backup.sh` |
| D3 | Restore drilled | 210 files, tree hash matches, sample byte-identical, RTO <1 s |
| D4 | Pre-write baselines | `fully_recoverable: true`, 0 uncommitted (was false, 9 files) |
| D5 | Revert point before each write | `git add` before the write; `test_revert_point.py` proves V2 recoverable and that without it V2 is lost |
| D6 | Crash-consistency scan | `verify` op; all four damage classes detected on planted fixtures |
| D7 | Backup freshness gated | gate step 18; fails when the last drill is >7 days old |

### Safe change

| # | what | evidence |
|---|---|---|
| C1 | Expand-only migration ladder | version 3; `test_migration.py` builds a real old-format index (14 columns) and migrates it |
| C2 | Corruption detect + repair | quarantines rather than deletes; healthy index deliberately untouched |
| C3 | Post-migration reconciliation | `documents 124 -> 124 IDENTICAL`; *derived tables are reported, not asserted* |
| C4 | Index-format contract | `bench/golden/format.json` + `bench/format_contract.py`; drift is a gate failure |

### Availability

| # | what | evidence |
|---|---|---|
| A1 | Degrade without the index | linear scan; measured **29 ms p50** degraded, abstains when no term discriminates |
| A2 | Read/write bulkhead | **measurement changed the conclusion** — see below |
| A3 | Jittered bounded retry | colliding writers back off to different instants |
| A4 | Per-session write budget | 50 writes, counted and logged; a loop guard, not a policy |

### Observability and capacity

| # | what | evidence |
|---|---|---|
| O1 | SLOs as enforced ratios | `bench/slo.py`; 5 objectives, a breach is a gate failure |
| O2 | Alarm age in status | `unresolved_alarms`, `oldest_unresolved_alarm` |
| O3 | Recoverability in status | read from the vault's own git |
| O4 | Latency histogram | per-op p50/p95, in memory, flushed with the session |
| P1 | Growth threshold | `bytes_per_document`, `growth_warn_at_documents` |
| P2 | Index vacuum | reclaimed **1,044 KB** from a 15.3 MB index; reports unless forced |
| P3 | Runbook | `RUNBOOK.md`, 11 procedures, every command verified by hand |
| P4 | Reclaim disk | 762 MB (`cargo clean`) + 2.2 GB of unused directories, operator-removed |

## Three milestones where measurement changed the answer

**A2 was specified as a read/write bulkhead. Measured first:**

```
readers during a full forced reindex : p50 12.1 ms vs 11.5 ms alone   (+5.1%)
worst case                           : 12.7 ms vs 12.8 ms             (no worse)
writer during a full forced reindex  : p50 9 ms                      (not blocked)
```

The index sets `journal_mode=WAL`, so readers take a snapshot and a writer does not fence them.
There was no contention to bulkhead, and a second code path would have been complexity bought with
nothing. What shipped instead is what the measurement supports: `test_isolation.py` asserts the
property and asserts `journal_mode=wal` directly, so a future change that removes either fails the
gate rather than quietly turning a 12 ms search into a multi-second one under load.

**C3's reconciliation was too strict at first.** The gate's own migration test compared
`doc_terms` and `term_df` across a full reindex and failed on a *correct* migration — those tables
are derived and rebuild freely. The invariant that matters is that the ALTER changes no rows, and
that is asserted inside `init_db`, between the columns being added and the version being written.
Asserting the wrong thing would have ended with the test weakened or switched off.

**A1's first version never abstained.** It scored every token equally, so a nonsense query became
`{a, not, term}` — each matching nearly every note — and it returned five confident results. Fixed
by mirroring the indexed path's discriminating-power thresholds. A search that cannot say "nothing"
is worse than one that is slow.

## Two bugs found by testing the tests

**The pre-write recovery command was wrong.** `git restore --source=:0` is not valid; the
index-restore is `git restore --` (source defaults to the index). Corrected in the source, and the
test now asserts both halves — with staging the pre-write content returns, without it the write is
lost — so the assertion cannot pass vacuously.

**`status` failed while the index was broken** — the one command you run *because* something is
wrong returned nothing at the moment its answer mattered most. A damaged header now reports
`readable: false`, `integrity: error: ...`, and what to do about it.

## What is still missing

The vault's own contract forbids cloud and sync, so D2 means **a second volume, an external disk,
or a local filesystem remote elsewhere on this machine.** Until that exists, the honest
description of this system is: *correct, fast, and one disk failure away from total loss.*
