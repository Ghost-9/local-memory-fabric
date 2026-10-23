# The three access gates

Rust makes separate decisions about whether a provider can access memory, what prior context may be sent to it, and whether it can write canonical notes.

| Gate | Loopback provider | Allowlisted remote provider | Other provider |
|---|---|---|---|
| Access | Allowed when provider and endpoint are configured | Read access for valid HTTP(S) endpoint | Denied |
| Outbound transcript | Kept | Local and tracked memory-derived context scrubbed by default | Scrubbed and access denied |
| Write | Allowed subject to proposal and Git safeguards | Propose-only by default | Denied |

A remote operator may explicitly set PI_MEMORY_REMOTE_CONTEXT_SCRUB=0 to retain transcript context or PI_MEMORY_REMOTE_FULL_ACCESS=1 to enable remote writes. These are separate choices. The write switch cannot authorize an unlisted provider.

Local provider identity alone is not enough: its endpoint must be HTTP(S) loopback. An unfamiliar hostname is not silently treated as local.

Scrub tracking overflow withholds uncertain tool results rather than evicting identifiers and re-admitting content. Init establishes a Git revert point or fails. Init and doctor explain that an empty vault returns no memories until Markdown notes are added.

The implementation and tests are in rust/src/extension.rs, rust/src/policy.rs, and rust/tests/protocol_conformance.rs. See docs/RUST-OPS.md for operation names.
