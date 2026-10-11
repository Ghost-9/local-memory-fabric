#!/usr/bin/env bash
# Single gate entrypoint. Every lane and reviewer runs exactly this.
# Order matters: cheapest and most decisive first, cross-engine agreement last but not least,
# and the benchmark harness as a regression tripwire.
# Must pass on stock macOS python3 (3.9): the Python engine avoids 3.10-only APIs.
set -euo pipefail
cd "$(dirname "$0")"

echo "== 1/11 python policy suite (security contract) =="
python3 test_memory_fabric.py

echo "== 2/11 extension policy suite (read, scrub and write gates) =="
node --experimental-strip-types --test remote-policy.test.ts

echo "== 3/11 context scrub suite (what may leave the machine) =="
# index.ts itself cannot be imported without the Pi runtime, which is why the scrub predicate
# was extracted to scrub.ts: this suite is the first coverage that file ever had.
node --experimental-strip-types --test scrub.test.ts

echo "== 4/11 typescript parses (syntax; no type checker assumed present) =="
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

echo "== 5/11 rust unit tests =="
( cd rust && cargo test --release --locked --quiet )

echo "== 6/11 python/rust parity =="
python3 rust/tests/parity.py

echo "== 7/11 retrieval determinism: identical request, identical bytes =="
# A regression guard for the prefix cache, not a proof of tie ordering -- see the docstring in
# bench/determinism.py for exactly what it can and cannot show.
python3 bench/determinism.py

echo "== 8/11 real-vault probe: the failure modes the synthetic corpus cannot see =="
# bench/recall.py scores 1.0 on a corpus generated from the terms it queries, so it cannot
# detect the failures this vault actually had (over-abstention, camelCase, acronyms). This runs
# the live vault read-only and fails if parity breaks. It reports, it does not assert a score -
# the score lives on the >=100-query fixture (#20), which does not exist yet.
if python3 bench/real_probe.py --json > /tmp/mf-probe.json 2>/dev/null; then
  python3 -c "
import json
d=json.load(open('/tmp/mf-probe.json'))
ans=sum(1 for r in d['rows'] if r['python']['shown']>0)
print(f\"  answered {ans}/{d['queries']} against the live vault, parity mismatches {d['parity_mismatches']}\")
raise SystemExit(1 if d['parity_mismatches'] else 0)"
else
  echo "  real-vault probe failed or the vault is not indexed; see bench/real_probe.py" >&2
  exit 1
fi

echo "== 9/11 gold fixture: >=100 queries, recall/MRR + no-hit precision =="
# bench/recall.py scores 1.0 on a corpus built from the terms it queries, so it cannot detect a
# ranking regression. bench/fixture_score.py runs 136 hand-written cases over a corpus built to
# BREAK retrieval: camelCase, two-letter acronyms, near-universal terms, paraphrase, and decoys
# made from each target's own words.
#
# The thresholds live in bench/fixture_gate.py so they are reviewable and diffable rather than
# buried in shell quoting. It asserts FLOORS, not equality, so ordinary variance does not make it
# flaky. no-hit precision is asserted too: a change that raises recall by answering everything is
# not an improvement.
if ! python3 bench/fixture_score.py --json > /tmp/mf-fixture.json 2>/dev/null; then
  echo "  GATE FAILURE: the gold fixture could not be run" >&2
  exit 1
fi
python3 bench/fixture_gate.py /tmp/mf-fixture.json

echo "== 10/11 benchmark harness: recall/MRR + exit-code taxonomy =="
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

echo "== 11/11 typescript type check (types, not just syntax) =="
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
