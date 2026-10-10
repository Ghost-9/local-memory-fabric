#!/usr/bin/env bash
# Single gate entrypoint. Every lane and reviewer runs exactly this.
# Order matters: cheap policy contract first, then the engine, then the
# Python/Rust agreement check that keeps the two engines honest.
# Must pass on stock macOS python3 (3.9): the Python engine avoids 3.10-only APIs.
set -euo pipefail
cd "$(dirname "$0")"

echo "== 1/5 python policy suite (security contract) =="
python3 test_memory_fabric.py

echo "== 2/5 extension policy suite (read, scrub and write gates) =="
node --experimental-strip-types --test remote-policy.test.ts

echo "== 3/5 typescript parses (syntax only; no type checker is installed) =="
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

echo "== 4/5 rust unit tests =="
( cd rust && cargo test --release --locked --quiet )

echo "== 5/5 python/rust parity =="
python3 rust/tests/parity.py

echo "== ALL GATES OK =="
