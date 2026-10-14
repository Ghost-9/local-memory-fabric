# Runbook

Procedures for the situations this system can actually be in. Every command here has been run;
none of it is aspirational. If a step's output does not match what is written, stop and say so
rather than continuing — a runbook that is followed past a wrong output is how small incidents
become large ones.

The one rule that matters when something is wrong: **the vault is the only irreplaceable thing.**
The index can be thrown away and rebuilt in under a second per 200 notes; the engine binary in
39 seconds; the vault cannot be rebuilt at all. When unsure, protect the vault and delete
everything else.

---

## 0. First response, whatever the symptom

```bash
cd ~/.pi/agent/extensions/local-memory-fabric
echo '{"op":"status"}' | ./rust/target/release/memory-fabric
```

Read these four fields first:

| field | meaning | worry if |
|---|---|---|
| `integrity` | SQLite's verdict on the index file | anything but `"ok"` |
| `readable` | whether the index could be opened at all | `false` |
| `fully_recoverable` | does every vault file have a committed baseline | `false` |
| `unresolved_alarms` | credential exposures not yet rotated | any number, older than a day |

`status` is written to answer while the index is broken. If it returns nothing at all, that is
itself the finding.

---

## 1. Search returns nothing, or something obviously wrong

**First, check whether it is degraded rather than empty.** A degraded result carries
`"degraded": true` and a `degraded_reason`. Degraded means the index was unavailable and search
fell back to a linear scan — answers are real but unranked.

```bash
echo '{"op":"search","query":"decision log","scope":"all","limit":3,"provider":"pi","loopback":true}' \
  | ./rust/target/release/memory-fabric | python3 -m json.tool | head -20
```

- `degraded: true` → go to **§2 (rebuild the index)**.
- `abstention: "no searchable terms"` → the query's terms appear in too many or too few notes to
  discriminate. Rephrase with a more specific term. This is correct behaviour, not a fault.
- Results present but wrong → go to **§4 (verify divergence)**.

---

## 2. Rebuild the index

Safe at any time. The index is derived data; this cannot lose vault content.

```bash
echo '{"op":"index","force":true}' | ./rust/target/release/memory-fabric
```

Expected: `added` + `updated` + `unchanged` sums to the note count, in roughly 42 ms per 200 notes.
A full forced rebuild of ~124 notes measures ~900 ms.

If it fails on a lock:

```bash
ps aux | grep -c '[m]emory-fabric'      # another request may be mid-flight
```

Wait a moment and retry. The engine retries a busy lock internally with a jittered backoff, so a
failure here means real contention, not a transient clash.

---

## 3. The index is corrupt

`status` reports `integrity` as anything but `"ok"`, or `readable: false`.

**Do not delete the file.** Quarantine and rebuild in one step — the bad file is the only evidence
of what went wrong, and it costs 13 MB to keep:

```bash
echo '{"op":"repair"}' | ./rust/target/release/memory-fabric
```

Expected output includes `"repaired": true`, a `quarantined` list naming the moved-aside file, and a
`rebuilt` block. Then re-run `status`: `integrity` should be `"ok"`.

A healthy index is deliberately **not** repaired by this command — it returns
`"repaired": false`. A rebuild triggered by a transient error would discard good data and hide the
cause.

Quarantined files look like `index.sqlite3.corrupt-<timestamp>`. They are safe to delete once the
cause is understood. Nothing deletes them automatically.

---

## 4. Search results and the vault disagree

```bash
echo '{"op":"verify"}' | ./rust/target/release/memory-fabric | python3 -m json.tool
```

Two blocks come back.

**`crash_consistency`** — structurally damaged notes. `damaged_total` must be 0 in a healthy vault.
Non-zero names the files and the kind:

| kind | what it means | what to do |
|---|---|---|
| `zero_byte` | file is empty | restore from git; a zero-byte note is never intentional |
| `not_utf8` | binary content in a Markdown file | inspect by hand; likely a bad write or a binary accidentally renamed |
| `unterminated_frontmatter` | starts with `---` and never closes it | the frontmatter is being parsed as body text; close it |
| `contradictory_dates` | `valid_to` before `valid_from` | the note is excluded from every `as_of` query; fix the dates |

This scan **never repairs**. Guessing at a half-written note is how the surviving half is lost.
Recovery is `git checkout-index -f -- <path>` (restores the file from the index — see §7) or
`git -C ~/system-memory log --oneline -- <path>`.

**`divergence`** — index versus disk. `on_disk_but_not_indexed` above 1 is worth a look; a small
number is normal, because notes matching the credential filter are deliberately excluded from the
index. `stale_mtime` should be 0 after an index run.

---

## 5. Restore the vault from backup

Use this when the vault directory is lost, corrupted, or is a machine that no longer exists.

```bash
cd ~/.pi/agent/extensions/local-memory-fabric
scripts/vault-backup.sh --list                 # what copies exist, with ages
scripts/vault-backup.sh --verify               # drills the newest, without touching the vault
```

`--verify` clones the newest bundle into a temporary directory and compares tree hashes against the
live vault. It does **not** overwrite anything, so it is safe to run at any time — and running it is
the point: a backup that has never been restored is a hope.

To actually restore, when the live vault is gone:

```bash
DEST="$HOME/Library/Mobile Documents/com~apple~CloudDocs/engram-vault-backup"
git clone "$(ls -1t "$DEST"/vault-*.bundle | head -1)" ~/system-memory-restored
```

Then inspect `~/system-memory-restored` before moving it into place. Never overwrite a surviving
vault with a restore — if the original is still there, its uncommitted state is newer than the
backup.

Measured RTO for the drill: **under 1 second** for a 210-file vault, 8.3 MB bundle.

---

## 6. Take a backup

```bash
scripts/vault-backup.sh
```

Rotation keeps 7 bundles and a hard 500 MB cap, evicting oldest first by whichever rule bites.
Override with `ENGRAM_BACKUP_DIR`, `ENGRAM_BACKUP_KEEP`, `ENGRAM_BACKUP_CAP_MB`.

The destination is iCloud Drive. **That is a deliberate trade**, recorded in RELIABILITY.md: an
off-machine copy in exchange for a third party holding one. Point `ENGRAM_BACKUP_DIR` at an
external disk to change it; nothing else needs to change.

**After any backup, run `--verify`.** The gate fails when the last successful drill is older than
7 days, so a stale or broken backup cannot pass quietly.

---

## 7. Recover a note that was just overwritten

Every permitted vault write is preceded by `git add`, so the exact pre-write content is in the
index.

```bash
git -C ~/system-memory checkout-index -f -- <path>     # restores the pre-write content
```

`git restore -- <path>` means the same thing but is blocked by the agent shell guard, so prefer
`checkout-index`. Both restore from the index; **neither restores the last commit**, which is the
point — the index holds what was there immediately before the write.

Verify what you are about to restore before restoring it:

```bash
git -C ~/system-memory show :0:<path>
```

---

## 8. Reclaim disk

```bash
echo '{"op":"vacuum"}'              | ...       # report: orphans, free pages, reclaimable bytes
echo '{"op":"vacuum","force":true}' | ...       # rewrite the file smaller
```

Measured: 1,044 KB reclaimed from a 15.3 MB index (89 free pages). VACUUM rewrites the whole file,
so it runs only when asked.

The engine binary and its build artefacts are usually larger than the index:
`cargo clean` in `rust/` reclaims ~762 MB; the next build takes 39 seconds.

---

## 9. Index format changed and something is complaining

```bash
echo '{"op":"schema"}' | ./rust/target/release/memory-fabric   # live format
python3 bench/format_contract.py                               # live vs recorded
```

Drift is a gate failure, not a warning. Migrations are expand-only — they add columns with
defaults and never rewrite, rename or drop — so there is no half-migrated state and no rollback
procedure: an older engine reads the same table and ignores columns it does not know.

If a migration fails, `init_db` returns an error naming the step and **does not advance the schema
version**. The next index run retries from the same rung. The reconciliation check (row counts
before and after must match) runs before the version is written.

---

## 10. Credential alarm

`status` reports `unresolved_alarms` and `oldest_unresolved_alarm`.

```bash
echo '{"op":"alarms"}' | ./rust/target/release/memory-fabric
echo '{"op":"resolve","alarm_id":"<id>"}' | ./rust/target/release/memory-fabric
```

The alarm stores a **fingerprint, never the value** — resolving it is a statement that the key was
rotated, not that it was found again. Only resolve after rotating.

An alarm that sits unresolved for weeks is worse than no alarm: it teaches everyone to ignore the
field.

---

## 11. Escalation

Stop and ask rather than improvising when:

- restoring would overwrite a vault that still exists,
- `status` reports damage that `repair` does not clear,
- a credential alarm needs resolving and the key has not been rotated,
- the only remaining backup is older than the data you would lose,
- anything requires deleting vault content.

**Do not delete vault content to make an error go away.** Every failure mode in this document has a
recovery that keeps the vault, and none of them require destroying notes.
