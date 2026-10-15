# Pre-registered evidence

The bar both competitors meet, and we did not.

Context Mode publishes 17 paired A/B results with equal-task-success checks and pre-registers each
run **before its first pair**. billion-context publishes 174,327 model calls over 4.5 months. We
published "78x fewer tokens per turn" from a cost model with no task-success check at all. That
number is arithmetic, not a result, and it should not have been stated as one.

## The rule

**Write the task list, the pair count, and the analysis down before the first run.** Then run it,
publish the outcome whatever it is, and never drop a pair.

Rationale: the only thing that separates a measurement from a story is that the analysis was chosen
before the data existed. Everything else — p-values, confidence intervals, "likely" ranges — is
decoration on top of that one commitment.

## What a result must carry

| field | why |
|---|---|
| task + success check | a cost saving with worse answers is a regression, and only a success check can tell you which you got |
| pairs, and the order | alternated order cancels provider load and cache drift across the run |
| per-arm token counts | from the provider's own usage, not the client's estimate |
| the interval | a point estimate from 8 pairs is not a finding |
| every valid pair | dropping the ones that "didn't work" is how a null result becomes a headline |

## Status: NOT RUN

There is no task suite yet, so nothing here has produced a number. Saying so is the point — a
directory named `evidence/` that quietly contains none is worse than not having it.

The first thing worth pre-registering is the **memory-injection** question, because it is the claim
this plugin actually rests on and it has never been tested:

> Does injecting the memory block improve task success, and at what token cost?

That requires a task suite where some tasks depend on prior knowledge held only in the vault. No
such suite exists. It is the same missing guard as the real-corpus ranking fixture — the two are
the same gap seen from different ends.
