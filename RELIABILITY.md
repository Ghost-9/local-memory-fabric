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

## What is still missing — and the one that matters most

| # | gap | status |
|---|---|---|
| **D2** | **No second copy of the vault.** Version history exists; there is one recovery site. Disk loss is total loss. | **needs an operator decision on where** |
| D3 | No restore has ever been drilled. A backup never restored is a hope. | blocked on D2 |
| D4 | 9 files are uncommitted, so their current state has no revert point. | policy written, not automated |
| D5 | We gate vault writes but do not perform them, so we cannot make a write atomic. | design only |
| D6 | A crash mid-write could leave a truncated note that still indexes. | not implemented |
| D7 | Nothing yet fails when the backup is stale. | not implemented |
| C1-C4 | Index schema migration, corruption auto-rebuild, reconciliation, format contract. | not implemented |
| A1-A2 | Read-only degradation, read/write bulkhead. | not implemented |
| P2 | Index vacuum for superseded notes. | not implemented |

The vault's own contract forbids cloud and sync, so D2 means **a second volume, an external disk,
or a local filesystem remote elsewhere on this machine.** Until that exists, the honest
description of this system is: *correct, fast, and one disk failure away from total loss.*
