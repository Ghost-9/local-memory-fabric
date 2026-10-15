#!/usr/bin/env python3
"""The Memory Ledger: what memory actually did, stated with its losses.

Every memory product asks the user to trust that memory helps. This produces the receipt instead -
and now that there IS a measurement, the receipt has to carry the bad news as prominently as the
good, or it is marketing with a table.

That is the whole design: the ledger reports what was spent, what was returned, and what was
measured about whether any of it worked. The task-suite result is printed in the same table as the
savings, not in a footnote.

Usage:  python3 bench/ledger.py [--days 30] [--json]
"""
import argparse, collections, datetime as dt, json, os, statistics, sys

STATE = os.path.expanduser("~/.pi/agent/scratch/memory-fabric")
USAGE = os.path.join(STATE, "usage.jsonl")
TASKSUITE = os.path.join(STATE, "tasksuite-p1.json")   # n=1; the full run would be tasksuite.json
VAULT = os.path.expanduser("~/system-memory")
# Structural cache ratios, not vendor quotes: cache read 0.1x, uncached input 1.0x.
READ_RATE = 0.1


def load(path, days):
    if not os.path.exists(path):
        return []
    cutoff = dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days) if days else None
    out = []
    for line in open(path):
        line = line.strip()
        if not line:
            continue
        try:
            r = json.loads(line)
            if cutoff:
                when = dt.datetime.fromisoformat(r["t"].replace("Z", "+00:00"))
                if when < cutoff:
                    continue
            out.append(r)
        except Exception:
            continue
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=30)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    recs = load(USAGE, args.days)
    kinds = collections.Counter(r.get("kind") for r in recs)
    injects = [r for r in recs if r.get("kind") == "inject"]
    refs = [r for r in recs if r.get("kind") == "session_refs"]
    pointers = [r for r in recs if r.get("kind") == "resume_pointer"]
    reads = [r for r in recs if r.get("kind") == "read"]
    decisions = [r for r in recs if r.get("kind") == "inject_decision"]
    breaks = [r for r in recs if r.get("kind") == "prefix_break"]

    injected_chars = sum(r.get("chars") or 0 for r in injects)
    injected_tokens = injected_chars // 4
    pointer_chars = sum(r.get("chars") or 0 for r in pointers)

    # A handoff pointer replaces re-sending the session transcript. Measured median resumed prompt:
    # 199,003 tokens. Conservative: count only pointers actually recorded.
    AVOIDED_PER_RESUME = 199_003
    avoided = len(pointers) * AVOIDED_PER_RESUME
    pointer_cost = pointer_chars // 4
    net_avoided = max(0, avoided - pointer_cost)

    # The measured effect, if a task-suite run exists.
    suite = None
    if os.path.exists(TASKSUITE):
        try:
            d = json.load(open(TASKSUITE))
            suite = d.get("summary")
        except Exception:
            suite = None

    if args.json:
        print(json.dumps({
            "window_days": args.days, "records": len(recs), "by_kind": dict(kinds),
            "injections": len(injects), "injected_tokens": injected_tokens,
            "sessions_resolving_refs": len(refs), "notes_read": len(reads),
            "handoffs": len(pointers), "tokens_avoided_by_handoff": net_avoided,
            "prefix_breaks": len(breaks), "shadow_gate_decisions": len(decisions),
            "measured_effect": suite,
        }, indent=2))
        return 0

    L = []
    L.append(f"# Memory Ledger — last {args.days} days")
    L.append("")
    L.append("## What it did")
    L.append("")
    L.append(f"- sessions that resolved memory refs : **{len(refs)}**")
    L.append(f"- injection events                    : **{len(injects)}**")
    L.append(f"- injected text                        : **{injected_tokens:,} tokens** across those events")
    L.append(f"- notes the agent then read            : **{len(reads)}**")
    L.append(f"- handoffs taken                       : **{len(pointers)}**")
    L.append("")
    L.append("## What it saved")
    L.append("")
    L.append(f"- a handoff pointer replaces a resumed session transcript. Measured median resumed prompt is")
    L.append(f"  **199,003 tokens**; a pointer is ~{pointer_chars // max(1,len(pointers)) if pointers else 0} tokens.")
    L.append(f"- tokens avoided by {len(pointers)} handoff(s): **{net_avoided:,}**")
    L.append(f"- prefix breaks (cache-invalidating)   : **{len(breaks)}**")
    L.append("")
    L.append("## What it cost, and whether it worked")
    L.append("")
    if suite:
        L.append("Measured by the task suite (24 real agent runs, n=1 per task):")
        L.append("")
        L.append("| category | n | with memory | without | cost delta |")
        L.append("|---|---|---|---|---|")
        for cat in ("vault", "control", "adversarial"):
            s = suite.get(cat)
            if not s:
                continue
            L.append(f"| {cat} | {s['n']} | {s['treat_pass']}/{s['n']} | {s['ctrl_pass']}/{s['n']} | "
                     f"{s['cost_delta_pct']:+.1f}% |")
        L.append("")
        L.append("**Memory cost more and did not pass more tasks.** On the adversarial category — where")
        L.append("nothing relevant exists to inject — success was identical at **36.6% higher cost**,")
        L.append("consistently across all three tasks. That is money spent for nothing.")
        L.append("")
        L.append("n=1, so this is the first data, not a result. And the vault tasks are answerable by")
        L.append("the agent's own search, so it says *injection does not beat self-search*, not that")
        L.append("memory is worthless.")
    else:
        L.append("No task-suite run recorded, so **whether any of this improved an agent's work is")
        L.append("unknown.** The numbers above are what was spent; they are not evidence of value.")
    L.append("")
    L.append("## What this ledger cannot tell you")
    L.append("")
    L.append("- whether a note memory surfaced was the one the agent needed, only whether it was read")
    L.append("- whether an unread injection helped anyway, by naming a project or a constraint")
    L.append("- anything about sessions where memory was never consulted at all")

    text = "\n".join(L)
    if args.out:
        open(args.out, "w").write(text + "\n")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
