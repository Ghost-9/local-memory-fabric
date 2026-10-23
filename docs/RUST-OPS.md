# Rust operation contract

The Pi extension sends newline-delimited JSON requests to the Rust executable with --serve. Each request has an op field. The response is one JSON object on one line. The full engine operation schema is also available through the MCP server’s tool listing.

The policy operations below are intentionally handled before ordinary engine dispatch.

| Operation | Purpose | Main request fields |
|---|---|---|
| provider-policy | Report allow, loopback, scrub, and write decisions together | model, optional local_providers |
| write-policy | Return the effective write mode for the current model | model |
| scrub-context | Filter an outbound transcript and report whether it removed anything or should notify | messages, now_ms, session_id |
| scrub-text | Redact credential-shaped text before remote export | text |
| injection-decision | Decide whether candidate search evidence is discriminative enough for unsolicited context | prompt, results |
| abstention-decision | Add a truthful abstention while preserving an engine reason | query, results, abstention, low_discrimination |
| scope-policy | Classify a note, match a project note, or validate a handoff | action, path, cwd, handoff |
| t3-adapter | Render a stable project slug or system instruction | action, workspace_path, project |
| session-cost | Track prompt tokens and return a notice only when policy says it is due | usage, threshold, interval, session_id |
| resume-pointer | Return a bounded resume pointer and staleness verdict | handoff, now_ms |
| engine-health | Record engine outcomes, check the breaker, and check the respawn budget | action, ok, now_ms, session_id |
| injection-account | Record injected characters and prefix changes or return session state | action, chars, refs, session_id |

The event operations used by the Pi host include before-agent-start, message-end, tool-guard, extension-command, create-handoff, project, and sync-agency. They are dispatched by the Rust engine and apply the same provider and scope rules as direct tools.

The engine also exposes compact and consolidate. Compact can create a new invariant note directly for a configured local provider when no target note exists; otherwise it produces a proposal. MCP marks compact as mutating. Consolidate defaults to preview, and a write request is checked by the Rust write gate.

## Policy invariants

- Local access requires both an explicitly configured local provider and an HTTP(S) loopback endpoint.
- A configured remote allowlist permits memory access only for a valid HTTP(S) endpoint. It does not make the endpoint loopback.
- Rust scrubs local turns and tracked memory-derived results from non-loopback outbound context by default. PI_MEMORY_REMOTE_CONTEXT_SCRUB=0 is the explicit opt-out.
- Remote write access is false unless PI_MEMORY_REMOTE_FULL_ACCESS is explicitly positive. This cannot authorize an unlisted provider.
- Scrub tracking overflow withholds uncertain tool results; it never evicts an identifier and silently re-admits the result.
- Handoffs are keyed by normalized project directory and require matching provenance immediately before injection.
- Search abstention never fabricates evidence when the engine returned none. Existing engine reasons remain authoritative.
- Index changes never replace Markdown as canonical memory.

## Example

~~~json
{"op":"provider-policy","model":{"provider":"openai","base_url":"https://api.openai.com/v1"}}
~~~

Consult the matching Rust unit and protocol/conformance tests when changing any operation contract.
