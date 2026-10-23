#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

if [ -f "$HOME/.cargo/env" ]; then
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "UNVERIFIED: Rust toolchain missing; cannot run the Engram gate." >&2
  exit 1
fi
if ! command -v node >/dev/null 2>&1; then
  echo "UNVERIFIED: Node.js missing; cannot load-check the host extension." >&2
  exit 1
fi

echo "== 1/6 Rust formatting =="
(cd rust && cargo fmt --all --check)

echo "== 2/6 Rust lint (warnings are errors) =="
(cd rust && cargo clippy --all-targets --all-features --locked -- -D warnings)

echo "== 3/6 Rust unit and integration tests =="
(cd rust && cargo test --all --locked)

echo "== 4/6 protocol, policy, migration, and installer conformance =="
(cd rust && cargo test --test protocol_conformance --locked)
bash tests/installer-contract.sh

echo "== 5/6 Pi extension load check =="
node --experimental-strip-types tests/extension-load.mjs

echo "== 6/6 README evidence contract =="
node tests/docs-contract.mjs

echo "== ALL GATES OK =="
