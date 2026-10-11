#!/usr/bin/env bash
# Single gate entrypoint. Every lane and reviewer runs exactly this.
# Order matters: cheapest and most decisive first, cross-engine agreement last but not least,
# and the benchmark harness as a regression tripwire.
# Must pass on stock macOS python3 (3.9): the Python engine avoids 3.10-only APIs.
set -euo pipefail
cd "$(dirname "$0")"

echo "== 1/7 python policy suite (security contract) =="
python3 test_memory_fabric.py

echo "== 2/7 extension policy suite (read, scrub and write gates) =="
node --experimental-strip-types --test remote-policy.test.ts

echo "== 3/7 typescript parses (syntax; no type checker assumed present) =="
node --input-type=module -e '
import { stripTypeScriptTypes } from "node:module";
import { readFileSync } from "node:fs";
let bad = 0;
for (const f of ["index.ts", "remote-policy.ts", "remote-policy.test.ts"]) {
	try { stripTypeScriptTypes(readFileSync(f, "utf8")); }
	catch (e) { bad++; console.error("  parse failed: " + f + ": " + e.message); }
}
process.exit(bad ? 1 : 0);
'

echo "== 4/7 rust unit tests =="
( cd rust && cargo test --release --locked --quiet )

echo "== 5/7 python/rust parity =="
python3 rust/tests/parity.py

echo "== 6/7 benchmark harness: recall/MRR + exit-code taxonomy =="
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

echo "== 7/7 typescript type check (types, not just syntax) =="
# index.ts is a security predicate file. A type error there ships silently, and no checker
# is installed in this repo, so borrow pi-webstack's TypeScript. The tsconfig and ambient
# stubs for Pi's modules live in /tmp, so the repo gains no node_modules and Pi's loader
# is unaffected. If either is missing, say so rather than silently skipping.
if [ -f /tmp/mf-tsc/tsconfig.json ]; then
  TSC="$(ls -d ~/.pi/agent/extensions/pi-webstack/node_modules/typescript/bin/tsc 2>/dev/null | head -1)"
  if [ -n "$TSC" ] && node "$TSC" -p /tmp/mf-tsc/tsconfig.json; then
    echo "  clean"
  else
    echo "  TYPE ERRORS above -- a type error in a security predicate is a real defect" >&2
    exit 1
  fi
else
  echo "  SKIPPED: /tmp/mf-tsc/tsconfig.json absent (stale /tmp); recreate it to enable" >&2
fi

echo "== ALL GATES OK =="
