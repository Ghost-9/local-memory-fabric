#!/usr/bin/env bash
# Single gate entrypoint. Every lane and reviewer runs exactly this.
# Order matters: cheapest and most decisive first, cross-engine agreement last but not least,
# and the benchmark harness as a regression tripwire.
# Must pass on stock macOS python3 (3.9): the Python engine avoids 3.10-only APIs.
set -euo pipefail
cd "$(dirname "$0")"

echo "== 1/9 python policy suite (security contract) =="
python3 test_memory_fabric.py

echo "== 2/9 extension policy suite (read, scrub and write gates) =="
node --experimental-strip-types --test remote-policy.test.ts

echo "== 3/9 context scrub suite (what may leave the machine) =="
# index.ts itself cannot be imported without the Pi runtime, which is why the scrub predicate
# was extracted to scrub.ts: this suite is the first coverage that file ever had.
node --experimental-strip-types --test scrub.test.ts

echo "== 4/9 typescript parses (syntax; no type checker assumed present) =="
node --input-type=module -e '
import { stripTypeScriptTypes } from "node:module";
import { readFileSync } from "node:fs";
let bad = 0;
for (const f of ["index.ts", "remote-policy.ts", "scrub.ts", "remote-policy.test.ts", "scrub.test.ts"]) {
	try { stripTypeScriptTypes(readFileSync(f, "utf8")); }
	catch (e) { bad++; console.error("  parse failed: " + f + ": " + e.message); }
}
process.exit(bad ? 1 : 0);
'

echo "== 5/9 rust unit tests =="
( cd rust && cargo test --release --locked --quiet )

echo "== 6/9 python/rust parity =="
python3 rust/tests/parity.py

echo "== 7/9 retrieval determinism: identical request, identical bytes =="
# A regression guard for the prefix cache, not a proof of tie ordering -- see the docstring in
# bench/determinism.py for exactly what it can and cannot show.
python3 bench/determinism.py

echo "== 8/9 benchmark harness: recall/MRR + exit-code taxonomy =="
# Fastest signal that retrieval quality or the exit-code contract regressed. Fails on
# recall@5 drift, any non-ok response, or an exit code the taxonomy does not promise.
python3 bench/recall.py | python3 -c 'import json,sys; r=json.load(sys.stdin); bad=[]; \
bad += [] if r.get("ok", True) else ["harness reported ok=false"]; \
bad += ["recall"] if r.get("python_recall_at_5") != 1.0 or r.get("rust_recall_at_5") != 1.0 else []; \
bad += ["protocol-errors"] if r.get("python_spawn_protocol_errors") else []; \
bad += ["exit-mismatch"] if r.get("python_spawn_exit_mismatches") else []; \
bad += ["order"] if r.get("result_order_mismatches") else []; \
print("  recall@5", r.get("python_recall_at_5"), "/", r.get("rust_recall_at_5"), \
      "| mrr@5", r.get("python_mrr_at_5"), "/", r.get("rust_mrr_at_5"), \
      "| exit mismatches", r.get("python_spawn_exit_mismatches")); \
sys.exit("GATE: benchmark harness regression: " + ",".join(bad) if bad else 0)'

echo "== 9/9 typescript type check (types, not just syntax) =="
# index.ts, remote-policy.ts and scrub.ts are security predicate files. A type error in one
# ships silently, so this step is not optional.
#
# It used to borrow tsc from a sibling extension with its config in /tmp, and print SKIPPED
# when either was missing — while still exiting 0 and printing ALL GATES OK. A reboot clears
# /tmp, so the type check could vanish behind a green gate with nothing on screen saying so.
# That is the silent-skip failure this repo keeps finding, so: the toolchain is vendored
# (see package.json), and a checker that cannot run is now a hard failure.
TSC="node_modules/typescript/bin/tsc"
if [ ! -f "$TSC" ]; then
  echo "GATE FAILURE: no vendored TypeScript at $TSC." >&2
  echo "  Run 'npm install' in $(pwd) once, then re-run the gate." >&2
  echo "  This step is deliberately fatal: a green gate that skipped the type check" >&2
  echo "  on a security predicate file is worse than a red one." >&2
  exit 1
fi
if node "$TSC" -p tsconfig.json; then
  echo "  clean"
else
  echo "  TYPE ERRORS above -- a type error in a security predicate is a real defect" >&2
  exit 1
fi

echo "== ALL GATES OK =="
