# Contributing (for human and AI agents alike)

This repository is built and reviewed by a mix of humans and AI agents. Everything an agent needs
to work here safely is in this file or in `ROADMAP.md` / `CHANGELOG.md`. If those three disagree,
the gate (`./test.sh`) is the arbiter and one of the docs is wrong — fix the doc.

## The one rule

**`./test.sh` must be green before a change is committed.** Not "mostly green". If it is red, the
change is not done. If the gate cannot run (missing tooling), say so out loud rather than
committing anyway — a silent skip is the failure mode this project keeps finding.

That is no longer hypothetical: the type-check step used to print `SKIPPED`, exit 0 and still
print `ALL GATES OK` when its checker was absent. It now **fails**. If step 9/9 tells you to run
`npm install`, run it — do not work around the step.

## Where to start

| I want to… | Read |
|---|---|
| understand the product | `README.md` |
| know what is being worked on | `ROADMAP.md` (phases, exit conditions) + issue `#14` (live to-do list) |
| know what shipped | `CHANGELOG.md` |
| work on retrieval quality | issue `#3` and `#4` — **do `#4` first**, `#3` depends on it |
| add or change a security decision | `remote-policy.ts` + `remote-policy.test.ts` (the gates) and `scrub.ts` + `scrub.test.ts` (what may leave the machine) |
| make a performance claim | `bench/` — measure first, claim second |

## Non-negotiables, and why they exist

1. **Never weaken the access policy to make something work.** Three gates (`remote-policy.ts`):
   may a provider *touch* the base, may it *see* results, may it *write*. A unknown hostname is
   never treated as loopback to unblock a session — announce the problem instead. Each of those
   decisions was made after a real failure.
2. **Never invent a fact.** If it was not measured, write *unmeasured*. A plausible number written
   once becomes a false fact forever.
3. **Keep raw tool output out of the context window.** Use `memory_fabric_sandbox`; do not paste
   large outputs into an issue, a commit message, or a diff.
4. **No credentials, no personal corpus statistics, no vault content** in issues, commits,
   changelogs or docs. This repo is public. Machine-specific corpus sizes are banded on purpose.
5. **Both engines must agree.** Rust and Python are checked against each other by
   `rust/tests/parity.py`. Change one without the other and parity fails — that is the point.
6. **Security predicates need a test, and `index.ts` cannot be one.** `index.ts` imports Pi's
   runtime, so no suite can import it — a whole class of defect can hide there. Anything deciding
   what leaves the machine therefore lives in `scrub.ts`, pure and testable. A dead `ctx.ui.notify`
   and a fail-open eviction both shipped green while `index.ts` had zero coverage (0.3.4); moving
   logic into a testable module is the fix, not adding a comment.

## Definitions of done

- Gate green, with the number of checks in each suite *stated* in the commit message.
- A new behaviour has a test that was **seen failing** before the fix. A test that has never failed
  is decoration.
- A performance claim names its measurement: n, median of round medians, and whether the delta sits
  inside the noise floor.
- A fix that changes user-visible behaviour is in `CHANGELOG.md`.

## Commit style

`area: what changed`, then the evidence. Reference the issue. If you retract an earlier finding,
say so — `ROADMAP.md` has a *Rejected on evidence* section for exactly this.
