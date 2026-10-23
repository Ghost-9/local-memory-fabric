# Measurements

The ranking-weight comparisons in bench/evidence/ordering_ab_results.json are the retained source for their metrics. That file records eight comparisons, with five paired observations per comparison. These ranking-fixture results do not measure end-user task success.

The prior latency report is preserved in bench/evidence/retired-latency-findings.md. Its raw per-run data was not retained, so it is historical and cannot be independently reproduced. It is not a current latency claim.

Memory injection's effect on task success and token cost is unmeasured. No estimate or cost model is presented as an outcome. A paired task suite with predetermined success checks is still required.

The Python measurement harness was retired. No replacement benchmark is currently included in the gate.
