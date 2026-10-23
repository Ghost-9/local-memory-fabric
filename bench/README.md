# Benchmark records

Python benchmark harnesses have been retired as part of removing Python from runtime, tests, and gates. They are not replaced by shell scripts that invoke Python. Findings are preserved under bench/evidence, together with the limits of the underlying records.

bench/baseline.sh remains as a Rust baseline-build helper. It is not a retrieval benchmark and does not reproduce the old Python A/B, scale, recall, determinism, or task-suite harnesses.

## Preserved results

- bench/evidence/ordering_ab_results.json contains recorded ranking-weight experiments. Its per-row values are the source for the retrieval-ranking notes in bench/evidence/README.md.
- bench/evidence/retired-latency-findings.md preserves the previous latency findings and states that raw per-run data was not retained. These figures are historical and non-reproducible; do not present them as current measurements.
- The task-success experiment for memory injection remains unrun. A task suite with a predefined success check and paired runs is required before making that claim.

New performance claims require retained raw inputs and outputs, a documented method, and a conformance check. Until a Rust-native benchmark harness retains those artifacts, latency results are unmeasured.
