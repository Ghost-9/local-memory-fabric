#!/usr/bin/env bash
# Build a baseline engine binary from any git ref, so any performance claim in
# this repo can be re-measured long after the original /tmp worktree is gone.
#
#   bench/baseline.sh 9665114            # -> prints the binary path
#   bench/ab.py "$(bench/baseline.sh 9665114)" rust/target/release/memory-fabric
#
# Uses `git archive` rather than a worktree so nothing is registered on the repo
# and nothing is left behind in `git worktree list`.
set -euo pipefail

ref="${1:?usage: baseline.sh <git-ref> [workdir]}"
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="${2:-/tmp/mf-baseline-$(printf '%s' "$ref" | tr -c 'A-Za-z0-9._-' '_')}"

rm -rf "$dir"
mkdir -p "$dir"
git -C "$root" archive "$ref" | tar -x -C "$dir"

echo "building baseline $ref in $dir" >&2
( cd "$dir/rust" && cargo build --release --locked >&2 )

echo "$dir/rust/target/release/memory-fabric"
