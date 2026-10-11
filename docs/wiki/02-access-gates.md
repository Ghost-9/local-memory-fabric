# The three access gates

There are **three** gates. They ask different questions, and conflating them is the mistake this
page exists to prevent.

| Gate | Question | Loopback | Operator-exempted | Everyone else |
|---|---|---|---|---|
| **1. Touch** (`providerAllowed`) | May this provider touch canonical memory at all? | yes | yes | **refused** |
| **2. See** (`shouldScrubContext`) | May memory-derived content reach its context? | yes | **yes** | scrubbed |
| **3. Write** (`mayWriteToMemory`) | May this provider write to canonical memory? | yes | only with `PI_MEMORY_REMOTE_FULL_ACCESS=1` | no |

Gate 1 is the access decision. Gate 2 is the scrub. Gate 3 is the write switch.

## Why three and not one

An earlier release described a single gate. Two bugs followed from the ambiguity:

- **An exempted provider's own results were stripped.** The scrub was keyed on "is the endpoint
  loopback", so the exemption let a provider *call* the fabric and then removed the answer before
  the model could read it. The call succeeded and the answer vanished.
- **Writes were blocked for everyone** with no way to distinguish "must not" from "not yet asked
  to", so the only way to grant write access was to delete the check.

## The rule that must not be bent

**An unknown hostname is never treated as loopback to unblock a session.** `localhost.localdomain`
and `0.0.0.0` resolve to this machine but are not in the loopback set, so a local model pointed at
them is *scrubbed*. That reads as "memory stopped working" — which is why the extension announces
the scrub once per process, naming the provider and the knobs and nothing else.

Widening the loopback set to silence that notice would trade a silence for a leak.

## The switches

| Variable | Effect |
|---|---|
| `PI_MEMORY_LOCAL_PROVIDERS` | which providers count as local (default `lmstudio,llama.cpp,ollama`) |
| `PI_MEMORY_REMOTE_ALLOWED_PROVIDERS` | the operator's exemption list (default `opencode-go,opencode,pi`) |
| `PI_MEMORY_REMOTE_FULL_ACCESS=1` | lets an *exempted* provider write; never reaches a provider not on the list |
| `enforce_export_policy` (engine config) | per-note export policy, off by default |

Every one of these is an **operator** decision. An agent must not add a provider, flip the
remote-write switch, or enable export enforcement on its own initiative.

The executable version of the table is `remote-policy.test.ts`. If this page and that file
disagree, the test file is right and this page is wrong.
