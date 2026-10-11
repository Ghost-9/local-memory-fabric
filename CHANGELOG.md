# Changelog

All notable changes to `local-memory-fabric`. The project is pre-1.0; anything may change, but the
gate (`./test.sh`) must be green before a change lands, and every performance claim carries its
measurement.

## [0.3.5] — 2026-10-12

Operator decisions recorded, beta blocker #9 closed, and the answer to "port it to Rust" —
which turned out to be mostly a misunderstanding worth writing down.

### Decisions (recorded, not just defaults)
- **`enforce_export_policy` stays OFF for 1.0**, and that is now a decision pinned by a test
  rather than an untested default. The reason is measured on this machine: **every** indexed note
  carries `deny_remote`, so turning enforcement on does not tighten a boundary — it hands an
  exempted provider **nothing at all**, removing the blanket read exemption wholesale. Doing it
  properly needs a per-note `allow_remote` convention, and which notes may leave this machine is
  the operator's decision, never an agent's. A test now asserts the default, the off-state, and
  both consequences. (#8)
- **The scrub notice repeats per session, rate-limited** (#10). Once per process was too little —
  reopen a scrubbed session and you saw it once, then silence, which is the symptom the notice
  exists to remove. Per request is worse: it becomes noise people dismiss. Now: the first scrub
  always announces, then at most one reminder per 10 minutes. `MEMORY_FABRIC_SCRUB_NOTICE_MS=0`
  silences the reminder; a negative value repeats every request. Five new tests, including one
  that simulates 200 requests over a long scrubbed session and asserts the count is periodic
  rather than per-turn.

### Fixed
- **The Python engine never received `loopback`** (commit `ae85f4a`). Found while writing the #8
  explanation above — immediately before the flag would have been flipped. With
  `enforce_export_policy` on, Python withheld `deny_remote` notes from **local** providers
  (shown=0, withheld=49) while Rust returned everything. Parity missed it because the test called
  the Python *method* directly, bypassing `dispatch_request`. Parity now exercises both `search`
  and `context` through the real JSON dispatch. 20 → 24 checks.

### Added
- **`install.sh`** — install / update / status / rollback / uninstall. Uninstall **never deletes
  memory**: plain uninstall leaves the state directory untouched, and `--purge` *moves* it to a
  timestamped sibling rather than deleting. Rollback keeps the previous tree and needs no network.
  Verified against a sandbox state directory, including that a purge leaves a readable backup.
- **Local version control for the vault** — the P0 gate for #11. The vault had no revert path; it
  now has a local git repo with **no remote**: a revert mechanism, not a publishing one, since the
  contract forbids sync and publish for this vault. Revert proven byte-identical on a real note.

### The Rust port: already done, and the real gap is elsewhere
Requested as "port the whole extension to Rust". Measured before acting: **all nine operations
already exist in Rust**, and the extension already uses that engine. Rust 12.9 ms vs Python 78.4 ms
per `search` on this machine — **6.2x** — because the Python path spawns a cold process per call
while Rust runs a persistent worker.

What is genuinely missing is not a port but **visibility**: `rust/target/` is gitignored, so a
fresh clone has no binary and `index.ts` **silently** falls back to the ~6x-slower engine. Same
silent-degradation shape as the type-check skip fixed in 0.3.4. `install.sh` now builds it and
says so on failure, and `status` names the live engine. The Pi-facing layer (`index.ts`) cannot be
ported to Rust — Pi loads TypeScript extensions — and does not need to be: it is a thin bridge that
spawns the engine.

## [0.3.4] — 2026-10-12

Three defects in the context scrub and the gate that guards it. All were found by reading the
code against the Pi runtime contract rather than by reading the docs, and **all three shipped
behind a green gate** — which is the finding worth recording, not just the fixes.

### Fixed
- **The scrub notice could never fire.** `index.ts` ended its `context` hook with an
  unconditional `return`, and the `ctx.ui.notify` announcing the scrub sat *below* it. The block
  was dead on arrival in the same commit that added it (`389eee3`), so the claim in 0.3.0 — "the
  extension now announces it once per process" — was never true. The provider it was written for
  (`localhost.localdomain`, `0.0.0.0` — local endpoints that are not in the loopback set) still
  gets scrubbed in silence, which is indistinguishable from memory being broken. The notice now
  runs before the return.
- **Overflow in the tracking set failed OPEN.** Past 4,096 tracked tool-call ids the set evicted
  its oldest entry. An evicted id makes an old tool result *untrackable*, which made it look safe,
  so content the policy had just decided to withhold was sent. Measured by replaying the hook: a
  session with 4,100 vault-touching tool calls leaked the 4 oldest results, and 4,146 leaked 50 —
  always at the front of the payload, so it invalidated the cached prefix too. The cap now trips
  a sticky `overflowed` flag and the scrub drops every tool result it cannot positively clear.
  Bounded memory costs some unrelated context; it never costs secrecy.
- **The type check could vanish behind a green gate.** Step 8 borrowed `tsc` from a sibling
  extension with its config in `/tmp`, and printed `SKIPPED` while still exiting 0 and printing
  `ALL GATES OK`. `/tmp` is cleared on reboot, so the type check on the security predicate files
  could disappear with nothing on screen saying so. The toolchain is now vendored
  (`package.json` + committed `tsconfig.json`), and a checker that cannot run is a hard failure.

### Added
- **`scrub.ts` + `scrub.test.ts`** — the scrub predicate, extracted into a pure module with 10
  tests. `index.ts` had **zero** test coverage: `remote-policy.test.ts` covers the gates, but the
  hook that *calls* them was unreachable from any suite. That gap is how all three defects above
  shipped green. The regression test was seen failing against the old evicting code before the fix.
- The scrub's contract, stated in one line and pinned by tests: **withhold more, never less.**

### Notes for reviewers
- The `context` hook is still cache-safe: it returns `undefined` when nothing was removed, and the
  filtered view grows append-only (60 simulated turns, 0 prefix shifts). The one prefix break is
  switching to a non-allowlisted provider mid-session, which is security-correct and unavoidable.
- `package.json` declares `pi.extensions: ["index.ts"]` so adding npm metadata cannot change how
  Pi discovers this extension. Pi skips `node_modules` when scanning, so the vendored dev
  dependencies are inert at runtime. The extension still has **no runtime dependency**.

## [0.3.0] — 2026-10-11

Beta-candidate release. The three access gates are explicit and tested, per-note export policy is
enforceable, large tool output can be kept out of the context window, and the failure modes that
used to be silent now say so.

### Added
- **`memory_fabric_sandbox`** — stores a large payload outside the canonical memory (mode 0600),
  indexes it, and returns only a bounded 480-character head plus a handle. Retrieval is the ordinary
  search/read path, so there is no second ranking policy to keep correct. Local credential filtering
  runs before anything is written. Payloads survive reindexing (`index()` protects the `sandbox/`
  prefix from its stale sweep). Known limitation documented in the README: payloads are
  `deny_remote`, so retrieving one under `enforce_export_policy` is loopback-only.
- **`enforce_export_policy`** config key — turns the previously unenforced per-row
  `export_policy` / `provider_policy` fields into an actual control. Off by default: enabling it
  overrides an operator's blanket read exemption, so it is a deliberate act, not a default. Applied
  identically in both engines to `search`, `context` and `read`, with a `withheld_export` count.
  The Rust engine re-reads the config per request, so flipping it needs no restart.
- **Exit-code taxonomy** in both engines — `0` returned content, `1` nothing to return,
  `2` refused by policy, `3` error — plus `--quiet` to suppress the JSON and keep the code.
  `--serve` is unaffected. An empty result is deliberately `1`, not `0`.

### Fixed
- **A locked index no longer masquerades as a broken one.** Both engines report
  `local memory index is busy; retry shortly [...]`; a genuinely malformed index is *not* dressed up
  as transient contention. Covered by unit tests plus an end-to-end check under an `EXCLUSIVE` lock.
- **Engine startup failures were invisible.** stderr was discarded by both spawn paths and `serve()`
  swallowed the init error with `.ok()` — the shape that hid a one-character config-writing bug for
  days. stderr is now captured, capped, and redacted for credential-shaped assignments.
- **The recall harness counted a no-match as a protocol error** once the exit-code taxonomy landed
  (any non-zero exit read as failure), so its two negative fixture cases reported
  `protocol_errors: 2` forever. It now classifies on the response body and separately checks that
  the exit code is one the taxonomy actually promises.
- **A fail-closed scrub was silent.** A provider that is neither loopback nor exempted lost the
  `vault-context` injection and every `memory_fabric_*` result with no message at all — including for
  a local endpoint named outside the loopback set (`localhost.localdomain`, `0.0.0.0`), which looks
  exactly like "memory stopped working". The extension now announces it once per process. The
  loopback set is deliberately **not** widened: an unknown hostname is never treated as local to make
  a session work.
  > **Corrected in 0.3.4:** the notice never actually fired — it sat below an unconditional
  > `return`. The scrub behaviour described above was real; the announcement was not.
  a session work.
- **`status` reports the live policy knobs** (`enforce_export_policy`) in both engines, so a
  configuration decision that changes what a session receives is visible.
- **Python 3.9 compatibility.** The engine called `Path.stat(follow_symlinks=...)`, which only
  exists from 3.10; a Homebrew `python@3.14` install removed the plain `python3` symlink and the
  whole suite failed against macOS's 3.9. Now `os.stat`, which takes `follow_symlinks` everywhere.

### Changed
- Three distinct access gates, extracted into a pure, tested module (`remote-policy.ts`):
  *read* access (loopback **or** operator-exempted), *see* access (an exempted provider's own
  fabric results are no longer stripped), and *write* access (loopback-only unless
  `PI_MEMORY_REMOTE_FULL_ACCESS` is set). Previously the README described one gate where there were
  three.
- Benchmarks consolidated to `bench/`; unused duplicates removed; the fixture generator is now
  importable rather than mis-filed.
- The gate runs the recall harness and a real `tsc --noEmit` pass, and **fails** on either a
  recall regression or an exit-code contract break.

## [0.2.0] — 2026-10-08

Initial public release.
