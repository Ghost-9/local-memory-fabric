#!/usr/bin/env bash
# Single gate entrypoint. Every lane and reviewer runs exactly this.
# Order matters: cheapest and most decisive first, cross-engine agreement last but not least,
# and the benchmark harness as a regression tripwire.
# Must pass on stock macOS python3 (3.9): the Python engine avoids 3.10-only APIs.
set -euo pipefail
cd "$(dirname "$0")"

echo "== 1/20 python policy suite (security contract) =="
python3 test_memory_fabric.py

echo "== 2/20 extension policy suite (read, scrub and write gates) =="
node --experimental-strip-types --test remote-policy.test.ts
# Engine breaker + respawn guard. These rules bound the cost of a broken engine: without them a
# failed worker costs a spawn and a timeout on every turn, and the only way to check the fix was
# to break a real engine and watch.
node --experimental-strip-types --test engine-health.test.ts
# #97/#105: the injected block must be byte-stable across turns or every cache prefix after the
# first change is void. Detecting that needs an account that can be told a size and asked whether
# it changed - which is why it is a module and not four variables inline.
node --experimental-strip-types --test injection.test.ts
# Session cost awareness. Thresholds derived from real telemetry (36,609 turns across 472 session
# files): 67.9% of turns carry a prompt >= 100K and carry 92.8% of all cache reads. The tests pin
# the fires-once-then-interval behaviour, because a notice that fires every turn gets muted and a
# muted notice is worse than none.
node --experimental-strip-types --test session-cost.test.ts
# Memory scoping. One of these tests IS the reported leak: a handoff for this project reaching a
# session for an unrelated one. A wrong note is worse than no note, because the receiving session
# cannot tell it is stale.
node --experimental-strip-types --test memory-scope.test.ts

echo "== 3/20 context scrub suite (what may leave the machine) =="
# index.ts itself cannot be imported without the Pi runtime, which is why the scrub predicate
# was extracted to scrub.ts: this suite is the first coverage that file ever had.
node --experimental-strip-types --test scrub.test.ts

echo "== 4/20 typescript parses (syntax; no type checker assumed present) =="
node --input-type=module -e '
import { stripTypeScriptTypes } from "node:module";
import { readFileSync } from "node:fs";
let bad = 0;
for (const f of ["index.ts", "remote-policy.ts", "scrub.ts", "remote-policy.test.ts", "scrub.test.ts"]) {
	try { stripTypeScriptTypes(readFileSync(f, "utf8")); }
	catch (e) { bad++; console.error("  parse failed: " + f + ": " + e.message); }
}
process.exit(bad ? 1 : 0);
'

echo "== 5/20 rust unit tests =="
( cd rust && cargo test --release --locked --quiet )

echo "== 6/20 python/rust parity =="
python3 rust/tests/parity.py

echo "== 7/20 retrieval determinism: identical request, identical bytes =="
# A regression guard for the prefix cache, not a proof of tie ordering -- see the docstring in
# bench/determinism.py for exactly what it can and cannot show.
python3 bench/determinism.py

echo "== 8/20 real-vault probe: the failure modes the synthetic corpus cannot see =="
# bench/recall.py scores 1.0 on a corpus generated from the terms it queries, so it cannot
# detect the failures this vault actually had (over-abstention, camelCase, acronyms). This runs
# the live vault read-only and fails if parity breaks. It reports, it does not assert a score -
# the score lives on the >=100-query fixture (#20), which does not exist yet.
if python3 bench/real_probe.py --json > /tmp/mf-probe.json 2>/dev/null; then
  python3 -c "
import json
d=json.load(open('/tmp/mf-probe.json'))
ans=sum(1 for r in d['rows'] if r['python']['shown']>0)
print(f\"  answered {ans}/{d['queries']} against the live vault, parity mismatches {d['parity_mismatches']}\")
raise SystemExit(1 if d['parity_mismatches'] else 0)"
else
  echo "  real-vault probe failed or the vault is not indexed; see bench/real_probe.py" >&2
  exit 1
fi

echo "== 9/20 gold fixture: >=100 queries, recall/MRR + no-hit precision =="
# bench/recall.py scores 1.0 on a corpus built from the terms it queries, so it cannot detect a
# ranking regression. bench/fixture_score.py runs 136 hand-written cases over a corpus built to
# BREAK retrieval: camelCase, two-letter acronyms, near-universal terms, paraphrase, and decoys
# made from each target's own words.
#
# The thresholds live in bench/fixture_gate.py so they are reviewable and diffable rather than
# buried in shell quoting. It asserts FLOORS, not equality, so ordinary variance does not make it
# flaky. no-hit precision is asserted too: a change that raises recall by answering everything is
# not an improvement.
if ! python3 bench/fixture_score.py --json > /tmp/mf-fixture.json 2>/dev/null; then
  echo "  GATE FAILURE: the gold fixture could not be run" >&2
  exit 1
fi
python3 bench/fixture_gate.py /tmp/mf-fixture.json

echo "== 10/20 benchmark harness: recall/MRR + exit-code taxonomy =="
# Fastest signal that retrieval quality or the exit-code contract regressed. Fails on
# recall@5 drift, any non-ok response, or an exit code the taxonomy does not promise.
python3 bench/recall.py | python3 -c 'import json,sys; r=json.load(sys.stdin); bad=[]; \
bad += [] if r.get("ok", True) else ["harness reported ok=false"]; \
bad += ["recall"] if r.get("python_recall_at_5") != 1.0 or r.get("rust_recall_at_5") != 1.0 else []; \
bad += ["protocol-errors"] if r.get("python_spawn_protocol_errors") else []; \
bad += ["exit-mismatch"] if r.get("python_spawn_exit_mismatches") else []; \
bad += ["order"] if r.get("result_order_mismatches") else []; \
print("  recall@5", r.get("python_recall_at_5"), "/", r.get("rust_recall_at_5"), \
      "| mrr@5", r.get("python_mrr_at_5"), "/", r.get("rust_mrr_at_5"), \
      "| exit mismatches", r.get("python_spawn_exit_mismatches")); \
sys.exit("GATE: benchmark harness regression: " + ",".join(bad) if bad else 0)'

echo "== 11/20 agent adapters (the first-class agent surface) =="
# adapters/ is the whole supported-agent surface - mcp.py, t3_code.py and the IDE JSON
# configs - and it had six passing tests that this gate never ran. Breaking every adapter
# left all steps green.
python3 test_adapters.py || { echo "  GATE FAILURE: an adapter is broken" >&2; exit 1; }

echo "== 12/20 cold-path A/B smoke (does the harness run and do both arms answer?) =="
# No latency threshold on purpose: wall-clock assertions flake in CI and would be turned off
# within a week. This proves the cold harness executes and both arms respond, which is the
# part that can silently rot. Compare numbers by hand with: python3 bench/ab_cold.py A B
B="$(ls -d rust/target/release/memory-fabric)"
python3 bench/ab_cold.py "$B" "$B" 2>&1 | tail -3 || { echo "  GATE FAILURE: cold A/B harness did not run" >&2; exit 1; }

echo "== 13/20 recorded retrieval contract (parity without the oracle) =="
# rust/tests/parity.py compares Rust to the Python engine live, which makes the oracle
# load-bearing for every parity check. This replays a recorded fixture instead, so the contract
# survives independently of any second binary.
python3 bench/parity_golden.py check || { echo "  GATE FAILURE: retrieval contract drifted" >&2; exit 1; }

echo "== 14/20 vault recoverability (is the memory base revertable?) =="
# Fails only when the vault is NOT under version control - that is a real failure, because
# nothing can be recovered. A dirty tree is normal during work, so it warns rather than fails;
# the authoritative check is engram status, which is also what makes this visible in use.
python3 - <<'PYCHECK'
import json, os, subprocess, sys
BIN = "rust/target/release/memory-fabric"
if not os.path.exists(BIN):
    print("  skipped: no engine built yet"); sys.exit(0)
out = subprocess.run([BIN], input='{"op":"status"}\n', capture_output=True, text=True, timeout=60)
try:
    st = json.loads(out.stdout)
except Exception:
    print("  skipped: status unavailable"); sys.exit(0)
h = st.get("vault_history") or {}
if not h.get("under_version_control"):
    print("  GATE FAILURE: the vault is not under version control; nothing is recoverable", file=sys.stderr)
    sys.exit(1)
dirty = h.get("uncommitted_files") or 0
print(f"  vault under version control at {h.get('head')}; {dirty} uncommitted file(s)")
if dirty:
    print(f"  note: {dirty} file(s) uncommitted, so their current state has no revert point (D4)")
print(f"  unresolved secret alarms: {st.get('unresolved_alarms', 0)} (oldest {st.get('oldest_unresolved_alarm')})")
print(f"  growth: {st.get('bytes_per_document')} B/doc, warns at {st.get('growth_warn_at_documents')} documents")
PYCHECK

echo "== 15/20 pre-write revert point (D5) and crash-consistency scan (D6) =="
# D5: proves the pre-write content of a vault write is recoverable, and that without the
# mechanism it is lost - so the assertion means something.
python3 test_revert_point.py || exit 1
# D6: scans the real vault for the shapes damage takes. Reports; does not repair.
echo '{"op":"verify"}' | "$B" > /tmp/mf-verify.json 2>/dev/null || true
python3 - <<'PYVER'
import json, os
p = "/tmp/mf-verify.json"
if not os.path.exists(p) or not open(p).read().strip():
    print("  verify produced no output; engine not built yet"); raise SystemExit(0)
d = json.load(open(p)); c = d["crash_consistency"]; g = d["divergence"]
print(f"  crash-consistency: damaged={c['damaged_total']} "
      f"(zero_byte={c['zero_byte']} not_utf8={c['not_utf8']} "
      f"unterminated_frontmatter={c['unterminated_frontmatter']} contradictory_dates={c['contradictory_dates']})")
print(f"  divergence: indexed={g['documents_indexed']} on_disk={g['files_on_disk']} "
      f"unindexed={g['on_disk_but_not_indexed']} stale_mtime={g['stale_mtime']}")
if c["damaged_total"]:
    print("  GATE FAILURE: the vault contains structurally damaged notes", file=sys.stderr)
    for k, v in c["sample"].items():
        if v: print(f"    {k}: {v}", file=sys.stderr)
    raise SystemExit(1)
PYVER

echo "== 16/20 reliability: availability (A1), corruption (C2), migration (C1/C3), isolation (A2) =="
# Each of these asserts something that can fail, and each uses the real index, so each copies the
# fixture aside and restores it.
python3 test_availability.py || exit 1
python3 test_migration.py || exit 1
python3 test_isolation.py || exit 1

echo "== 17/20 contract and objectives: index format (C4), SLOs (O1), index hygiene (P2) =="
python3 bench/format_contract.py || exit 1
python3 bench/slo.py || exit 1
# P2 reports; it rewrites 13 MB only when asked, so the gate reports rather than vacuums.
# The engine reads its request from stdin, so the JSON must be piped IN, not passed as an
# argument. Passing it as an argument sends an empty request and the engine answers
# "request must be valid JSON" - which is exactly what this line did at first.
echo '{"op":"vacuum"}' | "$B" | python3 -c '
import json,sys
d = json.load(sys.stdin)
# %-formatting, not an f-string: this runs under whatever python3 is on PATH, and 3.9 cannot
# nest the same quote type inside an f-string expression.
print("  index hygiene: documents=%d doc_terms=%d term_df=%d orphans=%d reclaimable=%dB"
      % (d["documents"], d["doc_terms"], d["term_df"], d["orphan_doc_terms"], d["reclaimable_bytes"]))
if d["orphan_doc_terms"] or d["orphan_term_df"]:
    print("  note: orphans present - run the vacuum command with force to reclaim them")
'

echo "== 18/20 consolidation: extractive, derived, non-destructive =="
# The verified finding was that notes only ever append, so superseded decisions accumulate forever.
# This proves consolidation reports what reading the corpus costs, and that a digest is DERIVED -
# writing one must not modify its source.
python3 - <<'PYCONS'
import json, subprocess, os, hashlib
BIN = "rust/target/release/memory-fabric"
def q(op):
    r = subprocess.run([BIN], input=json.dumps(op), capture_output=True, text=True)
    try: return json.loads(r.stdout)
    except Exception: return {}
rep = q({"op": "consolidate"})
print(f"  corpus: {rep.get('total_lines')} lines across {rep.get('notes_scanned')} notes; "
      f"{rep.get('notes_worth_a_digest')} notes have >=8 headings")
src = os.path.expanduser("~/system-memory/projects/local-memory-fabric-extension.md")
before = hashlib.sha256(open(src, "rb").read()).hexdigest() if os.path.exists(src) else None
d = q({"op": "consolidate", "ref": "projects/local-memory-fabric-extension.md", "write": True})
after = hashlib.sha256(open(src, "rb").read()).hexdigest() if os.path.exists(src) else None
if d.get("digest_path"):
    print(f"  digest: {d['source_lines']} -> {d['digest_lines']} lines at {d['digest_path']}")
if before and before != after:
    print("  GATE FAILURE: consolidation MODIFIED its source note", file=__import__("sys").stderr)
    raise SystemExit(1)
print("  source note unchanged by consolidation (non-destructive verified)")
PYCONS

echo "== 19/20 usage record and what it can answer (#95) =="
# The record exists to answer "what has this served". This step proves the viewer runs and that it
# refuses to invent the two numbers it cannot know (read-through, token cost).
python3 bench/usage_report.py --json > /tmp/mf-usage.json 2>&1 || {
  echo "  GATE FAILURE: bench/usage_report.py did not run" >&2; exit 1; }
python3 - <<'PYUSAGE'
import json
try:
    d = json.load(open("/tmp/mf-usage.json"))
except Exception:
    print("  usage record is empty; the viewer prints its own explanation rather than a zero")
    raise SystemExit(0)
print(f"  records={d['records']} kinds={d['by_kind']}")
print(f"  sessions_with_refs={d['sessions_with_refs']} injections={d['injections']} "
      f"injected_chars={d['injected_chars_total']} prefix_breaks={d['prefix_breaks']}")
if d["prefix_breaks"]:
    print(f"  GATE FAILURE: {d['prefix_breaks']} prefix breaks - the injected block changed mid-session",
          file=sys.stderr)
    raise SystemExit(1)
PYUSAGE

echo "== 19/20 backup freshness and the last restore drill (D7) =="
# A backup nobody checks is a hope. This reads the drill's own record and fails when it is older
# than the window, so a silently broken backup cannot pass CI. It does NOT re-run the drill -
# that belongs on a schedule, not in every gate.
python3 - <<'PYDRILL'
import datetime as dt, os, sys
d = os.environ.get("ENGRAM_BACKUP_DIR") or os.path.expanduser(
    "~/Library/Mobile Documents/com~apple~CloudDocs/engram-vault-backup")
rec = os.path.join(d, ".last-verify")
bundles = [f for f in (os.listdir(d) if os.path.isdir(d) else []) if f.startswith("vault-") and f.endswith(".bundle")]
if not bundles:
    print(f"  no backup bundle at {d}")
    print("  run: scripts/vault-backup.sh   (and then --verify)")
    sys.exit(0)          # absent backup is reported, not gated: a fresh clone has none
when, rto = None, "?"
if os.path.exists(rec):
    txt = open(rec).read().split()
    if txt:
        try: when = dt.datetime.strptime(txt[0], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)
        except Exception: when = None
    rto = next((t.split("=")[1] for t in txt if t.startswith("rto=")), "?")
age = (dt.datetime.now(dt.timezone.utc) - when).days if when else None
print(f"  {len(bundles)} bundle(s); newest drill: " + (f"{age}d ago, RTO {rto}s" if age is not None else "NEVER DRILLED"))
if age is None:
    print("  the backup has never been restored. A backup never restored is a hope (D3).")
elif age > 7:
    print(f"  GATE FAILURE: last successful restore drill was {age} days ago; window is 7", file=sys.stderr)
    sys.exit(1)
PYDRILL

echo "== 20/20 typescript type check (types, not just syntax) =="
# index.ts, remote-policy.ts and scrub.ts are security predicate files. A type error in one
# ships silently, so this step is not optional.
#
# It used to borrow tsc from a sibling extension with its config in /tmp, and print SKIPPED
# when either was missing — while still exiting 0 and printing ALL GATES OK. A reboot clears
# /tmp, so the type check could vanish behind a green gate with nothing on screen saying so.
# That is the silent-skip failure this repo keeps finding, so: the toolchain is vendored
# (see package.json), and a checker that cannot run is now a hard failure.
TSC="node_modules/typescript/bin/tsc"
if [ ! -f "$TSC" ]; then
  echo "GATE FAILURE: no vendored TypeScript at $TSC." >&2
  echo "  Run 'npm install' in $(pwd) once, then re-run the gate." >&2
  echo "  This step is deliberately fatal: a green gate that skipped the type check" >&2
  echo "  on a security predicate file is worse than a red one." >&2
  exit 1
fi
if node "$TSC" -p tsconfig.json; then
  echo "  clean"
else
  echo "  TYPE ERRORS above -- a type error in a security predicate is a real defect" >&2
  exit 1
fi

echo "== ALL GATES OK =="
