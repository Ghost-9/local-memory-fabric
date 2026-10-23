# Recorded and unmeasured evidence

This directory distinguishes retained records from claims that do not have retained raw data.

## Retrieval-ranking experiment

bench/evidence/ordering_ab_results.json contains eight recorded comparisons of ranking weights. Each comparison reports five paired observations (the n field), arm metrics, and median deltas. These are tuning records for the ranking fixture, not evidence that memory improves coding-task success. The JSON file is the source of every figure from this experiment.

For example, the recorded comparison that changed section weight from 0.15 to 0.0 reports Recall@3 of 0.9344 for both arms, with a median Recall@3 delta of 0.0. The associated MRR@3 values are 0.7354 and 0.7617. Read the source file for the other comparisons and metrics.

## Historical latency findings

bench/evidence/retired-latency-findings.md preserves previously published latency findings. The old harness did not retain its per-round raw data, and its Python implementation has been removed. Those figures are historical notes, not reproducible evidence or current performance claims.

## Injection task-success experiment: not run

There is no retained paired task suite that measures task success with memory on and off. Therefore this repository does not claim that injection improves task success or saves tokens while preserving task quality.

Before such an experiment, register the task set, pair count, ordering, and success checks. Retain every valid pair and per-arm usage. Publish the result regardless of direction. A cost estimate without task-success checks is not a measured outcome.
