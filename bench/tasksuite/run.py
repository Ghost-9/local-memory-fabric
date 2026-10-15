#!/usr/bin/env python3
"""The task suite: does injecting the memory block improve task success, and at what token cost?

This is the claim the product rests on and it has never been tested. Everything else in this repo -
the S1/3 standing, the latency numbers, the cost arithmetic - is about the ENGINE. This is about
whether the engine's output helps an agent, which is a different question and the only one a user
cares about.

Method, and why each part is there:

  PAIRED.  Every task runs twice, treatment and control, back to back. Pairing cancels the drift
           that would otherwise dominate: provider load, cache state, time of day.
  ALTERNATED. The arm order flips every pair, so a systematic advantage from running first or
           second cancels across the run instead of accumulating in one arm.
  ONE VARIABLE. The arms differ only by MEMORY_FABRIC_DISABLE_INJECTION=1. Same model, same prompt,
           same cwd, same tools.
  A SUCCESS CHECK PER TASK. A cost saving with worse answers is a regression, and only a success
           check can tell you which one you got. Every task carries a regex the answer must match.
  HARM IS MEASURED. Three task categories exist so the suite can show damage, not only benefit:
             vault       - answerable only from the vault; injection should help
             control     - general capability; BOTH arms should pass, or the harness is broken
             adversarial - nothing to do with the vault; injection is pure cost here
  NO PAIR IS DROPPED. Every valid pair counts, and the declared pair count is honoured.

Usage:
  python3 bench/tasksuite/run.py --smoke            # 2 tasks, 1 pair - proves the harness works
  python3 bench/tasksuite/run.py --pairs 3          # the real run
  python3 bench/tasksuite/run.py --pairs 3 --json   # machine-readable
"""
import argparse, json, os, re, statistics, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
TASKS = os.path.join(HERE, "tasks.json")
REPO = os.path.normpath(os.path.join(HERE, "..", ".."))
PI = os.environ.get("PI_BIN", "pi")


def run_one(task, inject, cwd, timeout):
    """One pi run over RPC. Returns (answer_text, usage, seconds, error)."""
    env = dict(os.environ)
    if not inject:
        env["MEMORY_FABRIC_DISABLE_INJECTION"] = "1"
    else:
        env.pop("MEMORY_FABRIC_DISABLE_INJECTION", None)

    proc = subprocess.Popen(
        [PI, "--mode", "rpc", "--no-session"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, cwd=cwd, env=env,
    )
    t0 = time.time()
    usage, text_parts, err = {}, [], None
    try:
        proc.stdin.write(json.dumps({"id": "p1", "type": "prompt", "message": task["prompt"]}) + "\n")
        proc.stdin.flush()
        deadline = t0 + timeout
        for line in proc.stdout:
            if time.time() > deadline:
                err = "timeout"; break
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except Exception:
                continue
            t = rec.get("type")
            if t == "message_update" and isinstance(rec.get("usage"), dict):
                usage = rec["usage"]
            elif t == "message_end":
                m = rec.get("message") or {}
                if m.get("role") == "assistant":
                    c = m.get("content")
                    if isinstance(c, str):
                        text_parts.append(c)
                    elif isinstance(c, list):
                        for b in c:
                            if isinstance(b, dict) and b.get("type") == "text":
                                text_parts.append(str(b.get("text", "")))
                    if isinstance(m.get("usage"), dict):
                        usage = m["usage"]
            elif t == "agent_settled":
                break
            elif t == "error":
                err = str(rec.get("message") or rec)[:160]
    finally:
        try:
            proc.stdin.close()
        except Exception:
            pass
        try:
            proc.terminate(); proc.wait(timeout=10)
        except Exception:
            try: proc.kill()
            except Exception: pass
    return "".join(text_parts), usage, time.time() - t0, err


def check(task, answer):
    return bool(re.search(task["check"], answer, re.IGNORECASE | re.DOTALL))


def cost_of(usage):
    """List-price units: uncached input at 1.0, cache read at 0.1, cache write at 1.25, output at 5."""
    u = usage or {}
    n = lambda k: float(u.get(k) or 0)
    return n("input") + 0.1 * n("cacheRead") + 1.25 * n("cacheWrite") + 5.0 * n("output")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pairs", type=int, default=3)
    ap.add_argument("--smoke", action="store_true", help="2 tasks, 1 pair, to prove the harness runs")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--cwd", default=REPO)
    ap.add_argument("--out", default=os.path.expanduser("~/.pi/agent/scratch/memory-fabric/tasksuite.json"))
    args = ap.parse_args()

    spec = json.load(open(TASKS))
    tasks = spec["tasks"]
    if args.smoke:
        tasks = [t for t in tasks if t["id"] in ("v1", "c1")]
        args.pairs = 1

    if not os.path.isdir(args.cwd):
        sys.exit(f"cwd does not exist: {args.cwd}")
    if subprocess.run(["bash", "-lc", f"command -v {PI}"], capture_output=True).returncode != 0:
        sys.exit(f"pi not found on PATH (looked for {PI}). Set PI_BIN.")

    print(f"task suite  {len(tasks)} tasks x {args.pairs} pair(s), {len(tasks)*args.pairs*2} runs")
    print(f"  cwd    : {args.cwd}")
    print(f"  arms   : treatment (injection on) vs control (MEMORY_FABRIC_DISABLE_INJECTION=1)")
    print()

    rows, skipped = [], 0
    for p in range(args.pairs):
        for task in tasks:
            # alternated order
            order = [True, False] if p % 2 == 0 else [False, True]
            got = {}
            for inject in order:
                ans, usage, secs, err = run_one(task, inject, args.cwd, args.timeout)
                got[inject] = {"answer": ans, "usage": usage, "secs": round(secs, 1),
                               "ok": check(task, ans), "err": err, "cost": round(cost_of(usage), 2)}
            treat, ctrl = got[True], got[False]
            if treat["err"] and ctrl["err"]:
                skipped += 1
                print(f"  pair {p+1} {task['id']:<4} SKIPPED  both arms errored: {treat['err']}")
                continue
            rows.append({"pair": p + 1, "id": task["id"], "category": task["category"],
                         "treatment": treat, "control": ctrl})
            mark = "ok " if treat["ok"] and ctrl["ok"] else ("HARM" if ctrl["ok"] and not treat["ok"]
                   else ("HELP" if treat["ok"] and not ctrl["ok"] else "fail"))
            print(f"  pair {p+1} {task['id']:<4} {task['category']:<12} {mark:<5} "
                  f"treat {'PASS' if treat['ok'] else 'fail'} {treat['cost']:>9,.0f}u  "
                  f"ctrl {'PASS' if ctrl['ok'] else 'fail'} {ctrl['cost']:>9,.0f}u")

    if not rows:
        print("\nno valid pairs - nothing can be concluded"); return 1

    def agg(sel):
        sel = [r for r in rows if sel(r)]
        if not sel: return None
        t_ok = sum(1 for r in sel if r["treatment"]["ok"])
        c_ok = sum(1 for r in sel if r["control"]["ok"])
        t_cost = statistics.mean(r["treatment"]["cost"] for r in sel)
        c_cost = statistics.mean(r["control"]["cost"] for r in sel)
        return {"n": len(sel), "treat_pass": t_ok, "ctrl_pass": c_ok,
                "treat_cost": t_cost, "ctrl_cost": c_cost,
                "cost_delta_pct": 100 * (t_cost - c_cost) / c_cost if c_cost else 0.0}

    print()
    print("  by category")
    print(f"    {'category':<13}{'n':>4}{'treat pass':>12}{'ctrl pass':>11}{'treat cost':>13}{'ctrl cost':>11}{'delta':>9}")
    summary = {}
    for cat in ("vault", "control", "adversarial"):
        s = agg(lambda r, c=cat: r["category"] == c)
        if not s: continue
        summary[cat] = s
        print(f"    {cat:<13}{s['n']:>4}{s['treat_pass']:>8}/{s['n']:<3}{s['ctrl_pass']:>8}/{s['n']:<3}"
              f"{s['treat_cost']:>13,.0f}{s['ctrl_cost']:>11,.0f}{s['cost_delta_pct']:>+8.1f}%")

    out = {"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "pairs": args.pairs,
           "skipped_pairs": skipped, "rows": rows, "summary": summary, "smoke": args.smoke}
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    json.dump(out, open(args.out, "w"), indent=1)

    print()
    print("  reading, in advance, so the result cannot be reinterpreted afterwards:")
    print("    control arm failing vault tasks  -> the harness or the model is broken; fix before judging")
    print("    vault: treatment > control       -> injection helps, and by how much")
    print("    adversarial: cost up, success flat -> injection is pure cost where it is not needed")
    print("    vault: treatment == control      -> no measured benefit; the claim is unsupported")
    print(f"\n  written to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
