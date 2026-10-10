# Local Memory Fabric (v0.2.0 Rust engine)

A local-only memory API over approved Markdown sources. Obsidian remains canonical; the generated SQLite/FTS5 database is a disposable retrieval index. The Rust engine is model-free, lexical, project-scoped, provenance-bearing, and review-only. Obsidian remains the only canonical durable store; QMD remains unchanged and independent.

## Components

- `rust/` — primary Rust engine, linked to bundled SQLite with FTS5; `--serve` keeps one JSONL worker alive per Pi process. Runtime performs no network access.
- `memory_fabric.py` — Python reference/fallback implementation; standard library plus SQLite FTS5.
- `index.ts` — Pi adapter. Five collision-resistant `memory_fabric_*` tools are deferred; Rust is selected by default when the release binary exists, otherwise Python is used. Requests use JSON over stdin, never shell interpolation.
- `test_memory_fabric.py` and `rust/tests/parity.py` — isolated policy tests and Python/Rust parity checks using synthetic Markdown fixtures.
- `rust/benchmark.py` and `benchmark_fixture.py` — aggregate-only evaluators on a generated synthetic 12-case, 111-document corpus; no personal vault data is used.

## Scope and data flow

By default, only Markdown under `agents/`, `company/`, `daily/`, `evidence/`, `pi-harness/`, and `projects/`, plus root `README.md`, `ROADMAP.md`, `STATUS.md`, and `Welcome.md`, is considered. Hidden directories, symlinks, Apple Notes/export-named paths, excluded directories, non-Markdown files, and any Markdown note matching the local 11-pattern credential heuristic are skipped. A frontmatter `classification: secret|restricted|excluded` also removes the note from the generated index. All other indexed content is `local_only` by default.

The index and review proposals live under `~/.pi/agent/scratch/memory-fabric/`, not in the vault. The directory is mode `0700`; SQLite and proposal files are mode `0600`. Indexing is explicit (`/memory-fabric index` or CLI `index`); it updates only the generated index. It never changes or deletes a canonical Markdown file.

Each indexed source records scope, classification, source path and content hash, source mtime, index timestamp, explicit frontmatter lifecycle (`status`, `valid_from`, `valid_to`, `superseded_by`), provider policy (`loopback_allowlist_only`), and export policy (`deny_remote`). Missing lifecycle metadata remains `unspecified`; the module does not infer that a note is current. With explicit `as_of` (ISO date), `search` enforces validity bounds and can include superseded/archive/deprecated sources only when the dates support the historical query. Without `as_of`, those statuses are excluded, validity bounds are returned with a warning, and the host clock is not used to decide truth. Invalid temporal metadata is excluded; overlapping lifecycle records are warned about rather than automatically resolved. `indexed_at` and filesystem mtimes use the host clock and are audit metadata only, not temporal truth.

`search` uses SQLite FTS5/Porter stemming plus a rare-term overlap gate. Empty or weak matches abstain. A rank is not a probability, a match is not a fact, and the source passage remains authoritative. `context` is bounded by an approximate character budget; the `4 chars/token` conversion is only a transport cap, not a tokenizer measurement. Embeddings, model inference, and graph extraction are absent. There is no standalone daemon or network service; the Rust worker is launched on demand and lives only with the Pi process.

## Local provider policy

Access is **default-deny** and controlled by the operator. A provider may receive memory excerpts,
paths, or injected vault context only when it passes one of two gates:

1. **Local (the default):** `ctx.model.provider` is in `PI_MEMORY_LOCAL_PROVIDERS` (default
   `lmstudio,llama.cpp,ollama`) **and** the model's configured `baseUrl` is loopback
   (`localhost`, `127.0.0.1`, or `::1`, HTTP/S). Nothing leaves the machine.
2. **Operator-exempted cloud provider:** `ctx.model.provider` is in
   `PI_MEMORY_REMOTE_ALLOWED_PROVIDERS` (default `opencode-go,opencode,pi`). These deliberately skip
   the loopback test, so retrieved excerpts, note paths, titles and scope names are sent to that
   provider's own endpoint. Set the variable to a comma-separated list to change the exemption, or
   to an empty string to restore loopback-only behaviour.

Everything else is refused with a block response, including an unlisted cloud provider and a request
that asserts no provider at all. Recall quality is identical either way; what changes is where the
bytes go.

**Who decides: the operator, never the agent.** An agent must not add itself, its own provider, or
any other provider to the exempted list, and must not ask another agent to do so. Writes to
canonical memory stay blocked for every provider regardless of this setting, so no model can change
the vault.

The existing `vault-context.ts` auto-injection is subject to exactly the same two gates as the tools. Before a request to a provider that is not allowlisted, the adapter removes prior assistant/tool-result messages generated by configured local providers and strips `vault-context` custom messages, so a provider switch inside a Pi session cannot carry local-only context forward. Set `PI_MEMORY_LOCAL_PROVIDERS` to a comma-separated exact provider list if your local provider uses another ID.

The collision-resistant Pi tools are named `memory_fabric_search`, `memory_fabric_context`, `memory_fabric_read`, `memory_fabric_propose`, and `memory_fabric_proposal_show`; their prefix avoids colliding with other extensions such as `pi-hermes-memory`. A remote call to `memory_fabric_propose` may stage candidate text, but receives only the proposal ID and cannot see the generated diff. Remote providers that are neither loopback nor operator-exempted receive a generic block response without paths or excerpts. Pi also blocks all explicit Apple Notes/export references, and blocks direct `write`/`edit` to canonical memory. Use proposals and inspect them locally.

**Security boundary:** these are adapter-level controls, not an OS sandbox or a universal egress firewall. An arbitrary shell command, a symlink alias, a custom tool/MCP server, another AI client's unguarded file tool, or user-authored text can bypass or independently disclose data. A hard guarantee requires separate OS permissions/sandboxing and provider-specific adapters. A loopback URL can itself be a proxy to a hosted model, so its configuration must also be trusted. The portable CLI accepts a `provider` assertion; non-Pi clients must derive that value from trusted runtime configuration and independently verify the endpoint and routing. Never pass `provider: "ollama"` merely because it is convenient. The operator exemption above is the one setting that lets vault bytes reach a remote endpoint, so treat it as a security decision: record the reason, keep the list as short as the work allows, and prefer a loopback model when one will do.

## First use in Pi

Build the Rust binary once, then start/reload Pi. If the binary is missing, engine selection falls back to Python unless `MEMORY_FABRIC_ENGINE=rust` explicitly requires Rust.

```sh
cd ~/.pi/agent/extensions/local-memory-fabric/rust
cargo build --release --locked
```

The binary is `rust/target/release/memory-fabric`. Build the local index explicitly:

```text
/memory-fabric status
/memory-fabric index
/memory-fabric proposals
```

The memory tools are deferred to avoid default prompt overhead; Pi's `tool_search` can discover them when needed. `/memory-fabric tools` reports their registered names and exposure. Their content-bearing operations work only in a session whose provider is allowlisted: loopback, or
explicitly exempted by the operator. The company scope is available for future curated `company/` Markdown; no company facts are invented or added here.

## Portable local API / other clients

The Python reference API accepts one JSON request on stdin. The Rust engine accepts newline-delimited JSON on stdin in `--serve` mode and returns one JSON response per line. Neither API makes network calls. Non-Pi adapters must provide trusted provider identity and enforce their own endpoint policy; the Rust core alone cannot identify the active model provider.

```sh
python3 ~/.pi/agent/extensions/local-memory-fabric/memory_fabric.py index
printf '%s' '{"op":"search","query":"prompt cache","scope":"system:pi-harness","provider":"ollama"}' \
  | python3 ~/.pi/agent/extensions/local-memory-fabric/memory_fabric.py --json-stdin
```

Supported JSON operations: `status`, `index`, `search` (optional `as_of`), `context` (optional `as_of`), `read`, `propose`, `proposal-list`, and `proposal-show`. The portable layer is an API contract, not a ready-made adapter for every AI client. Client adapters should pass the trusted active provider and check that its endpoint is loopback,
unless the operator has explicitly exempted that provider, in which case returned excerpts may reach
it and nothing else.

Useful environment settings:

- `MEMORY_FABRIC_VAULT` — canonical Markdown root (default `~/system-memory`).
- `MEMORY_FABRIC_HOME` — generated index/proposal root (default `~/.pi/agent/scratch/memory-fabric`).
- `MEMORY_FABRIC_PYTHON` — Python executable for fallback (default `python3`).
- `MEMORY_FABRIC_ENGINE` — `auto` (default), `rust`, or `python`.
- `MEMORY_FABRIC_BIN` — override Rust binary location.
- `PI_MEMORY_LOCAL_PROVIDERS` — comma-separated exact provider IDs; applies to the Pi adapter and both engines.
- `PI_MEMORY_FABRIC=0` — disable this Pi extension.

The first invocation writes a mode-0600 `config.json` under the generated-state directory. Review it locally before expanding include paths. Never add Apple Notes or their exports.

## Review-only write-back

`memory_fabric_propose` accepts an existing project note and existing Markdown heading, validates indexed source references, runs an 11-pattern credential heuristic locally, and stores a bounded proposal plus unified diff under `proposals/`. It does not apply. The 11-pattern credential filter is heuristic, not a guarantee; review the complete diff before manually making any durable edit. There is deliberately no apply command in this release.

## Verification

Run tests from this directory:

```sh
python3.13 test_memory_fabric.py -v
(cd rust && cargo test --locked)
python3.14 rust/tests/parity.py
python3.14 rust/benchmark.py
```

The Rust benchmark reports Python/Rust ranking parity, fresh-index times, persistent-query latency, and worker RSS using generated synthetic content. Its 12 cases remain too small to establish production-grade recall or abstention.

The fixture benchmark prints aggregate metrics only. Do not send fixture text, source passages, local proposals, or vault contents to a remote model, search engine, or MCP server.
