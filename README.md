# Engram

Engram is a local memory engine for coding agents. The repository keeps the historical name local-memory-fabric because existing extension paths may refer to it.

The Markdown vault is canonical and human-editable. SQLite/FTS is a disposable index that can be rebuilt from the vault. Rust owns retrieval, persistence, provider authorization, redaction, context injection, abstention, scope, proposals, handoffs, and session policy.

The package version has one source: rust/Cargo.toml. The Pi extension remains TypeScript because the host loads index.ts. That file only registers tools and hooks, serializes requests to the Rust process, and renders responses. It contains no memory or privacy policy and package.json declares no npm runtime dependencies.

## Install and use

Build the Rust engine and install the Pi extension:

~~~sh
source "$HOME/.cargo/env"
cargo build --release --locked --manifest-path rust/Cargo.toml
./install.sh install
~~~

The installer builds from source whenever Cargo is available. Without Cargo, it accepts only a release binary whose --version matches the package version in Cargo.toml. A mismatch stops installation with a build instruction.

For an existing vault, run the engine’s init operation once, then check it with doctor. Init prepares the state and vault, initializes Git for versioned writes, and reports when no Markdown notes exist. An empty vault returns no memories until notes are added. The index can then be built with the index operation.

The Pi extension registers tools for context, search, read, propose/apply, sandbox, audit, alarms, links, sections, project memory, handoff, consolidation, and health. The Rust engine also serves MCP over stdio.

## Access and privacy

The Rust policy has three separate gates:

| Gate | Decision |
|---|---|
| Access | A loopback endpoint from the configured local-provider list, or a provider explicitly on the remote allowlist, may access memory. Unknown providers and endpoints fail closed. |
| Context | Rust removes local-provider turns and tracked vault-derived content from the outbound transcript for non-loopback endpoints by default, including allowlisted remote providers. Set PI_MEMORY_REMOTE_CONTEXT_SCRUB=0 only when the operator explicitly wants to keep that context. |
| Write | Allowlisted remote providers are propose-only by default. PI_MEMORY_REMOTE_FULL_ACCESS=1 explicitly enables remote writes. A provider outside the allowlist cannot write through this switch. |

Proposals stay outside the canonical vault until applied. The local compact operation may create a new invariant note directly when none exists; MCP advertises it as mutating, and it is available only to a configured local provider. Init versions the vault with Git; if Git cannot be initialized, init fails rather than leaving canonical writes without a revert point. The index is never the source of truth.

Transcript scrubbing protects prior session context when the active endpoint is remote. Direct tool results remain readable by a provider that passed the explicit memory-access allowlist; that access decision is separate from the transcript scrub.

Relevant settings include PI_MEMORY_LOCAL_PROVIDERS, PI_MEMORY_REMOTE_ALLOWED_PROVIDERS, PI_MEMORY_REMOTE_CONTEXT_SCRUB, PI_MEMORY_REMOTE_FULL_ACCESS, MEMORY_FABRIC_VAULT, MEMORY_FABRIC_HOME, and MEMORY_FABRIC_BIN. Rust owns their interpretation.

## Rust operations

The extension calls Rust operations for policy and event handling. The complete contract and request fields are documented in docs/RUST-OPS.md. The policy operation family includes provider-policy, write-policy, scrub-context, scrub-text, injection-decision, abstention-decision, scope-policy, t3-adapter, session-cost, resume-pointer, engine-health, and injection-account. The Pi hooks submit before-agent-start, message-end, tool-guard, extension-command, create-handoff, project, and sync-agency events.

## Verification

The single gate is:

~~~sh
./test.sh
~~~

It runs Rust formatting, Clippy with warnings denied, all Rust unit and integration tests, protocol and installer conformance, and a Pi extension load check. A missing tool or failed step stops the gate; a step is never silently skipped. No Python runtime, Python test path, npm test framework, or npm runtime install is part of the product.

## Measurements

This README makes no performance claim. Recorded retrieval-ranking experiments are in bench/evidence/ordering_ab_results.json. Historical latency findings whose original per-run records were not retained are clearly marked as non-reproducible in bench/evidence/retired-latency-findings.md. The injection task-success experiment remains unmeasured; see bench/evidence/README.md.

## Project files

- rust/src contains the engine and policy implementation.
- rust/tests contains protocol and migration conformance tests.
- index.ts is the Pi host transport.
- install.sh installs or removes the extension without deleting memory by default.
- bench/evidence retains recorded results and the status of unmeasured work.
- MIGRATION.md maps the former Python and TypeScript logic to Rust.
- CHANGELOG.md records shipped changes; ROADMAP.md lists current work.
