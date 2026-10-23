# Retrieval evidence plan

Ranking-weight records currently retained in bench/evidence/ordering_ab_results.json are fixture experiments, not end-user task results. Historical latency findings are preserved separately, but their original per-round records are missing.

Future retrieval changes should:

- Define a fixture and success metric before changing ranking behavior.
- Keep the Markdown corpus canonical and use isolated temporary vaults.
- Compare serialized result sets and preserve deterministic tie-breaking.
- Retain each run and its inputs under bench/evidence.
- Report null or negative outcomes without dropping pairs.

The former Python measurement harness has been retired. A new Rust-native benchmark harness is not yet part of the gate. Until it retains run artifacts, latency changes are unmeasured.
