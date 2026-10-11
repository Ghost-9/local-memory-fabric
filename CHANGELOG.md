# Changelog

All notable changes to `local-memory-fabric`. The project is pre-1.0; anything may change, but the
gate (`./test.sh`) must be green before a change lands, and every performance claim carries its
measurement.

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
