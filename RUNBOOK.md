# Runbook

## Build and install

~~~sh
source "$HOME/.cargo/env"
cargo build --release --locked --manifest-path rust/Cargo.toml
./install.sh install
~~~

The installer prefers a source build whenever Cargo is available. If Cargo is unavailable, it downloads the platform artifact for the Cargo package version, verifies the checksum, and checks the binary’s --version. A mismatch is an error.

## First setup

Run init through the JSON-lines server, then inspect doctor and status:

~~~sh
printf '%s\n' '{"op":"init"}' | rust/target/release/memory-fabric --serve | jq .
printf '%s\n' '{"op":"doctor"}' | rust/target/release/memory-fabric --serve | jq .
printf '%s\n' '{"op":"status"}' | rust/target/release/memory-fabric --serve | jq .
~~~

Init creates the configured vault if needed, initializes Git for versioned writes, and reports that an empty vault returns no memory. Add Markdown notes and index them before expecting search results.

## Rebuild the derived index

~~~sh
printf '%s\n' '{"op":"index","force":true}' | rust/target/release/memory-fabric --serve | jq .
printf '%s\n' '{"op":"verify"}' | rust/target/release/memory-fabric --serve | jq .
~~~

Rebuilding the index does not change Markdown source files.

## Run the gate

~~~sh
./test.sh
~~~

The gate requires Cargo and Node for the Pi load check. It reports a named UNVERIFIED reason and exits nonzero if either is missing. Python and npm are not part of the gate.

## Provider policy

Configure local providers with PI_MEMORY_LOCAL_PROVIDERS and remote read access with PI_MEMORY_REMOTE_ALLOWED_PROVIDERS. Rust scrubs outbound transcripts for non-loopback endpoints by default, including allowlisted remote providers. PI_MEMORY_REMOTE_CONTEXT_SCRUB=0 is an explicit opt-out. Remote writes remain propose-only until PI_MEMORY_REMOTE_FULL_ACCESS=1. An unlisted provider cannot be granted write access by this setting.

## Empty memory or unavailable index

- An empty vault is an expected empty result, not a successful memory response. Init and doctor explain that notes must be added.
- If the index is stale or corrupt, use doctor and verify first. Reindex only the derived database.
- Do not copy private vault content into repository issues, logs, or benchmark artifacts.
