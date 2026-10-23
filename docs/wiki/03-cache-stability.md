# Deterministic responses

The protocol is expected to return byte-stable JSON for identical requests against unchanged inputs. Search ties use deterministic ordering, generated references do not contain request-time metadata, and the integration suite compares serialized responses.

Context injection is Rust-owned and session-scoped. Project notes are selected only for matching projects; global tiers remain cross-cutting. The index is disposable and may be rebuilt from Markdown without altering notes.

A passing test establishes only the behavior it exercises. It is not evidence of provider cache hit rate, latency, or task success. The retained ranking experiment is in bench/evidence/ordering_ab_results.json. Historical latency findings without raw runs are marked non-reproducible in bench/evidence/retired-latency-findings.md.

When response behavior changes, add or update a Rust test and preserve deterministic tie-breaking.
