#!/usr/bin/env bash
# Single gate entrypoint. Every lane and reviewer runs exactly this.
# Order matters: cheap policy contract first, then the engine, then the
# Python/Rust agreement check that keeps the two engines honest.
# Must pass on stock macOS python3 (3.9): the Python engine avoids 3.10-only APIs.
set -euo pipefail
cd "$(dirname "$0")"

echo "== 1/3 python policy suite (security contract) =="
python3 test_memory_fabric.py

echo "== 2/3 rust unit tests =="
( cd rust && cargo test --release --locked --quiet )

echo "== 3/3 python/rust parity =="
python3 rust/tests/parity.py

echo "== ALL GATES OK =="
