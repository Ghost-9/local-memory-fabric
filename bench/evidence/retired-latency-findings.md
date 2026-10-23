# Retired latency findings

This file preserves the results previously published in bench/README.md when the harness depended on Python. The per-round raw timing records were not retained. The original README described the first run as alternating A/B with one thousand observations per arm and a later run as two hundred observations for the real index. The figures below are transcribed from that historical report, not recalculated.

| Historical corpus | Previously reported baseline p50 | Previously reported current p50 | Previously reported change |
|---|---:|---:|---:|
| Small personal corpus, banded | 2.75 ms | 1.11 ms | -59.6% |
| Synthetic corpus, 1,130 documents and 3,707 term rows | 2.89 ms | 2.80 ms | -3.1% |
| Synthetic corpus, 1,130 documents and about 40,000 distinct terms | 7.57 ms | 1.11 ms | -85.4% |
| Later real-index rerun | 2.70 ms | 1.22 ms | -54.7% |
| Later synthetic rerun, 1,130 documents and 41,970 terms | 7.07 ms | 1.00 ms | -85.8% |

The historical interpretation was that the improvement tracked vocabulary size more than document count. That finding is retained, but the absent raw pairs mean these numbers cannot be independently reproduced from this repository. The Python harness has been removed, and this record must not be used as a current benchmark claim.
