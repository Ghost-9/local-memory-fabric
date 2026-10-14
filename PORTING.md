# What belongs in Rust, what belongs in Python

Written 2026-10-15, after the strip pass. Every figure here is measured on this machine.

## The state, stated plainly

The repository holds **two implementations of one contract**:

| | lines | speed | role |
|---|---|---|---|
| `rust/src/*.rs` | 6,174 | 2.2 ms cold search | **shipped** — the extension spawns it, `engram` is a symlink to it |
| `memory_fabric.py` | 1,775 | 55 ms one-shot | **test oracle** — executed only by the gate |

`ROADMAP.md` previously described the Python engine in the past tense, as though it had already
been removed. It has not. The Agency extension removed its Python path; this one has not.

## What could move to Rust

**Nothing needs to.** The engine already exists in both. There is no shipped behaviour in Python
that Rust lacks. The one genuine port candidate is:

| candidate | lines | why it is a candidate | verdict |
|---|---|---|---|
| `adapters/mcp.py` | 352 | it is **shipped** code an agent runs, and it needs a Python runtime present. XERJ ships `xerj mcp` as a subcommand of the same static binary — one artefact, no runtime. | **port** — the only real one |
| `adapters/t3_code.py` | 60 | thin, shipped, same argument | port with the above |

That would make the distributed artefact a **single binary with no Python at all** for the
shipped path, which is the property that matters for installation.

## What should stay Python, permanently

| stays | lines | why Python is correct here |
|---|---|---|
| `bench/*.py` | 1,525 | measurement tooling. It is rewritten constantly, read by humans, and never shipped. A build step would cost more than it saves. |
| `rust/tests/parity.py` | 282 | it must drive **both** engines; writing it in either one would bias it. |
| `test_memory_fabric.py` | 610 | the oracle's own tests. |
| `test_adapters.py` | 91 | adapter contracts. |

**Roughly 2,500 lines of Python are test and measurement infrastructure.** Porting them to Rust
would be pure cost: they are not on any shipped path.

## The one real decision: keep the oracle, or record it?

Three options, honestly stated.

**A — keep `memory_fabric.py` forever.** Parity is checked against a live second implementation,
so it can catch *new* divergences, not only old ones. Cost: 25% of the codebase, a second
semantics to keep in step, and a slow Python step in the gate.

**B — delete it, as Agency did.** Cheapest and fastest. You keep 33 parity checks' worth of
confidence you already have, and lose the ability to detect a divergence introduced tomorrow.

**C — record it, then delete it (recommended).** Capture the oracle's outputs across the fixture
corpus as a **golden file**, gate against that, then remove the oracle. Parity becomes a
regression check against recorded expectations rather than a live comparison.

| | A | B | C |
|---|---|---|---|
| catches new divergences | yes | no | no |
| catches regressions | yes | no | **yes** |
| gate cost | Python in the loop | cheapest | cheapest |
| codebase | +1,775 lines | −1,775 lines | −1,775 lines |
| risk | drift between two engines | silent divergence | expectations frozen at a point in time |

**A cost I understated when first writing this file.** Deleting the oracle also deletes
`test_memory_fabric.py` - and inspecting it properly shows those are not parity tests. They are
the **security contract**: scope isolation, export-policy enforcement, credential alarms, sandbox
secret refusal, traversal rejection, and the read staleness check. 28 tests.

The Rust engine has **26 unit tests, most of them pure helpers** - paths, tokenize, exit codes,
busy messages. It has **no behavioural suite of its own.** The Python suite is the closest thing,
and it tests Python.

So the loss from deleting the oracle is larger than "one cross-check": it removes the only
behavioural coverage, and `rust/tests/parity.py` would have nothing to compare against. Measured:

| after deleting Python | remaining coverage |
|---|---|
| lost | 28 behavioural tests, 33 live parity checks |
| kept | 26 Rust unit tests, 136-case recall fixture, determinism check, adapters, and the recorded contract above |

**The order therefore matters, and it is not "record then delete":**

1. record the cross-engine contract as a fixture - **done** (`bench/golden/parity.json`, 544 cases,
   captured only because both engines agreed on all of them);
2. **port the 28 behavioural tests to Rust** - this is the real milestone, because the security
   contract must outlive the engine that happens to test it;
3. and only then delete the oracle, which by that point is genuinely just 1,775 lines of parity
   reference with no unique coverage.

**Why C over B:** deleting the oracle and the check together is how a codebase loses its ability to
notice anything. C keeps the detection that matters most — *did this change break parity?* — at a
fraction of the cost, and states its limit honestly: it cannot see a divergence that never existed
when the golden file was written.

**Why not A, given the two bugs it found:** those bugs were found during a period when the two
implementations were actively diverging, which is exactly the period that ends. The oracle's value
declines as the engines converge; its cost does not.

## Not yet done

- `adapters/mcp.py` is not ported. It remains the only shipped Python.
- No golden parity fixture exists. Option C is a proposal, not a plan that has been executed.
- The 4 open items from the strip pass: `.pi-lens` and `.openviking` (2.2 GB, unused, removal needs
  the operator because the guard refuses recursive deletes in the home tree), and
  `secret_alarms: 1` unresolved in the index.
