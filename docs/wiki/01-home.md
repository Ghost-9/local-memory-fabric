# Local Memory Fabric

Local-first, review-first memory retrieval for Pi. A canonical Markdown base, a disposable
SQLite/FTS5 index, an optional Rust worker, and a review-only write path.

**This wiki is the operational reference.** The repo's `README.md` is the product description,
`ROADMAP.md` the phases, and `CHANGELOG.md` what shipped. This wiki holds how-to and the rules
that are easy to break by accident.

## The five rules that are easy to get wrong

1. `./test.sh` green, or it is not done. A silent skip is the failure mode this project keeps
   finding.
2. Never widen the access policy to make a session work. Announce the problem instead.
3. Keep large tool output out of context — `memory_fabric_sandbox`, not a paste.
4. Nothing personal in this repo. It is public: no credentials, no corpus statistics, no vault
   content.
5. Rust and Python must agree. `rust/tests/parity.py` fails when they do not; that is the point.

## Where things live

| Path | What it is |
|---|---|
| `index.ts` | the Pi extension (one file) |
| `remote-policy.ts` + `.test.ts` | the access decision table, pure and tested |
| `memory_fabric.py` | Python engine |
| `rust/` | Rust worker, built by `bench/baseline.sh` for any commit |
| `bench/` | the measurement harness |
| `test.sh` | the gate |

Full contributor guidance, including definitions of done: `AGENTS.md` in the repo.
