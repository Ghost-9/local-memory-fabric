#!/usr/bin/env python3
"""Synthetic parity checks for the Python reference engine and Rust JSONL engine."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

MODULE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(MODULE))
from memory_fabric import MemoryFabric, dispatch_request  # noqa: E402

BINARY = Path(__file__).resolve().parents[1] / "target" / "release" / "memory-fabric"


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def main() -> int:
    if not BINARY.exists():
        print(json.dumps({"ok": False, "error": "build Rust release binary first"}))
        return 2
    mismatches = 0
    checks = 0
    with tempfile.TemporaryDirectory(prefix="memory-fabric-parity-") as temp:
        base = Path(temp)
        vault = base / "vault"
        py_home = base / "python-state"
        rust_home = base / "rust-state"
        write(vault / "projects/atlas.md", "---\nstatus: current\nclassification: local_only\nvalid_from: 2024-01-01\n---\n\n# Atlas project\n\n## Decisions\n\nThe local retrieval service uses SQLite FTS5 and preserves source provenance.\n\n## Log\n\n- First entry.\n")
        write(vault / "projects/atlas/legacy.md", "---\nstatus: superseded\nvalid_from: 2020-01-01\nvalid_to: 2024-01-01\nsuperseded_by: projects/atlas.md\n---\n\n# Legacy Atlas retention\n\nHistoric retention snapshots used the retired vault format.\n")
        write(vault / "projects/atlas/current-policy.md", "---\nstatus: current\nvalid_from: 2022-01-01\n---\n\n# Current Atlas policy\n\nCurrent retention snapshots replaced the old archive rules.\n")
        write(vault / "pi-harness/cache.md", "# Prompt cache behavior\n\n## Cache\n\nChanging an early prompt section can invalidate the cached conversation prefix. Append context to preserve reuse.\n")
        write(vault / "company/policy.md", "# Company privacy policy\n\n## Rules\n\nDurable memory changes require human review and approval.\n")
        write(vault / "Apple Notes/not-for-index.md", "PRIVATE SENTINEL\n")
        candidate = "CANDIDATE" + "X" * 24
        write(vault / "projects/sensitive.md", f"# Sensitive\n\nSUPABASE_SERVICE_ROLE_KEY={candidate}\n")
        before = {str(p.relative_to(vault)): p.read_bytes() for p in vault.rglob("*.md")}
        py = MemoryFabric(home=py_home, vault=vault)
        rust_env = os.environ.copy()
        rust_env["MEMORY_FABRIC_HOME"] = str(rust_home)
        rust_env["MEMORY_FABRIC_VAULT"] = str(vault)
        rust_env["PI_MEMORY_LOCAL_PROVIDERS"] = "lmstudio,llama.cpp,ollama"
        proc = subprocess.Popen([str(BINARY), "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, text=True, env=rust_env)

        def rust(req):
            proc.stdin.write(json.dumps(req, ensure_ascii=False) + "\n")
            proc.stdin.flush()
            return json.loads(proc.stdout.readline())

        try:
            py_index = py.index()
            rust_index = rust({"op": "index"})
            checks += 1
            mismatches += int(py_index.get("documents") != rust_index.get("documents") or
                              py_index.get("excluded_sensitive") != rust_index.get("excluded_sensitive"))

            queries = [
                {"op": "search", "query": "SQLite FTS5 retrieval provenance", "scope": "project:atlas", "limit": 5, "provider": "ollama"},
                {"op": "search", "query": "legacy retention snapshots", "scope": "project:atlas", "limit": 5, "provider": "ollama"},
                {"op": "search", "query": "legacy retention snapshots", "scope": "project:atlas", "limit": 5, "as_of": "2023-06-01", "provider": "ollama"},
            ]
            for req in queries:
                p = py.search(req["query"], scope=req["scope"], limit=req["limit"], provider=req["provider"], as_of=req.get("as_of"))
                r = rust(req)
                checks += 1
                p_refs = [x["source_ref"] for x in p.get("results", [])]
                r_refs = [x["source_ref"] for x in r.get("results", [])]
                mismatches += int(p_refs != r_refs or p.get("temporal_warning") != r.get("temporal_warning"))

            py_read = py.read("projects/atlas.md", provider="ollama", heading="Decisions")
            rust_read = rust({"op": "read", "source_ref": "projects/atlas.md", "provider": "ollama", "heading": "Decisions"})
            checks += 1
            mismatches += int(py_read.get("content") != rust_read.get("content") or py_read.get("source_ref") != rust_read.get("source_ref"))

            py_remote = py.search("private source content", provider="anthropic")
            rust_remote = rust({"op": "search", "query": "private source content", "provider": "anthropic"})
            checks += 1
            mismatches += int(set(py_remote) != set(rust_remote) or py_remote.get("blocked") != rust_remote.get("blocked"))

            py_context = py.context("atlas", "SQLite FTS5 provenance", "ollama", token_budget=100)
            rust_context = rust({"op": "context", "project": "atlas", "query": "SQLite FTS5 provenance", "provider": "ollama", "token_budget": 100})
            checks += 1
            mismatches += int(py_context.get("source_count") != rust_context.get("source_count") or
                              py_context.get("abstention") != rust_context.get("abstention"))

            py_prop = py.propose("atlas", "projects/atlas.md", "Log", "- Candidate pending human review.",
                                 ["pi-harness/cache.md"], provider="ollama")
            rust_prop = rust({"op": "propose", "project": "atlas", "target": "projects/atlas.md", "section": "Log",
                              "text": "- Candidate pending human review.", "source_refs": ["pi-harness/cache.md"], "provider": "ollama"})
            checks += 1
            mismatches += int(py_prop.get("ok") != rust_prop.get("ok") or py_prop.get("status") != rust_prop.get("status"))
            py_show = py.proposal_show(py_prop["proposal_id"], provider="ollama")
            rust_show = rust({"op": "proposal-show", "proposal_id": rust_prop["proposal_id"], "provider": "ollama"})
            checks += 1
            mismatches += int(py_show.get("status") != rust_show.get("status") or
                              ("diff" not in py_show) != ("diff" not in rust_show))

            after = {str(p.relative_to(vault)): p.read_bytes() for p in vault.rglob("*.md")}
            checks += 1
            mismatches += int(before != after)
            # Per-note export policy must behave identically in both engines. Every note here
            # is deny_remote, so a non-loopback caller must see nothing and get a withheld
            # count; the same query asserting loopback must see everything.
            for home in (py_home, rust_home):
                cfg = home / "config.json"
                data = json.loads(cfg.read_text(encoding="utf-8"))
                data["enforce_export_policy"] = True
                cfg.write_text(json.dumps(data), encoding="utf-8")
            # A fresh Python engine for the enforcing phase: the one above read its config at
            # construction. Rust re-reads the file per request, which is the point.
            py_export_engine = MemoryFabric(home=py_home, vault=vault,
                                            config={"enforce_export_policy": True})
            py_export = py_export_engine.search("retention snapshots", scope="all", provider="ollama", limit=5)
            rust_export = rust({"op": "search", "query": "retention snapshots", "scope": "all",
                                "provider": "ollama", "limit": 5})
            checks += 1
            mismatches += int(py_export.get("results") != rust_export.get("results") or
                              py_export.get("withheld_export") != rust_export.get("withheld_export") or
                              bool(py_export.get("results")))
            py_local = py_export_engine.search("retention snapshots", scope="all", provider="ollama",
                                               limit=5, loopback=True)
            rust_local = rust({"op": "search", "query": "retention snapshots", "scope": "all",
                               "provider": "ollama", "limit": 5, "loopback": True})
            checks += 1
            mismatches += int([r["source_ref"] for r in py_local["results"]] !=
                              [r["source_ref"] for r in rust_local["results"]])
            # REGRESSION (0.3.5): the block above calls the Python METHOD, which bypasses
            # dispatch_request - and that is exactly where the bug lived. The Python engine
            # accepted `loopback` and never received it from the JSON path, so with enforcement
            # on it withheld deny_remote notes from LOCAL providers too, while Rust (which does
            # forward it) returned everything. Same request, opposite results, parity green.
            #
            # So: exercise search and context through the real JSON dispatch, with loopback
            # asserted, and require both engines to return the same thing.
            for op, payload in (
                ("search", {"op": "search", "query": "retention snapshots", "scope": "all",
                            "provider": "ollama", "limit": 5, "loopback": True}),
                ("context", {"op": "context", "query": "retention snapshots",
                             "provider": "ollama", "loopback": True}),
            ):
                py_dispatch = py_export_engine.__class__(
                    home=py_home, vault=vault, config={"enforce_export_policy": True})
                py_via_dispatch = dispatch_request(py_dispatch, payload)
                rust_via_dispatch = rust(payload)
                # `context` has no top-level withheld_export; it reports source_count instead.
                py_shown = (py_via_dispatch.get("shown") if op == "search"
                            else py_via_dispatch.get("source_count"))
                rust_shown = (rust_via_dispatch.get("shown") if op == "search"
                              else rust_via_dispatch.get("source_count"))
                checks += 1
                mismatches += int(py_shown != rust_shown)
                # And the substantive claim: a loopback caller must still SEE the notes,
                # and must not be blocked, under enforcement.
                checks += 1
                mismatches += int(not py_shown or bool(py_via_dispatch.get("blocked")) or
                                  bool(rust_via_dispatch.get("blocked")) or
                                  (op == "search" and py_via_dispatch.get("withheld_export")))
            checks += 1
            # Both engines must land a sandboxed payload identically: same handle (content
            # addressed, so identical bytes imply an identical digest), same bounded head, and
            # the same retrievability. A drift here would mean one engine's sandbox silently
            # duplicates or drops payloads.
            #
            # Searches here assert loopback on purpose: sandboxed payloads are deny_remote, and
            # the earlier block has export enforcement on. Storing is open to an exempted
            # provider, but retrieving under enforcement is not - a documented limitation,
            # not something to paper over by marking payloads allow_remote.
            sandbox_text = "shard three replicated log lines\n" * 40
            py_sb = py.sandbox("big tool output", sandbox_text, provider="ollama")
            rust_sb = rust({"op": "sandbox", "title": "big tool output", "text": sandbox_text,
                            "provider": "ollama"})
            checks += 1
            mismatches += int(py_sb.get("handle") != rust_sb.get("handle") or
                              len(py_sb.get("head", "")) != len(rust_sb.get("head", "")) or
                              py_sb.get("chars") != rust_sb.get("chars") or
                              bool(py_sb.get("ok")) is False)
            py_found = py.search("shard replicated", scope="all", provider="ollama", limit=5, loopback=True)
            rust_found = rust({"op": "search", "query": "shard replicated", "scope": "all",
                               "provider": "ollama", "limit": 5, "loopback": True})
            checks += 1
            mismatches += int(
                py_sb.get("handle") not in [r["source_ref"] for r in py_found.get("results", [])] or
                rust_sb.get("handle") not in [r["source_ref"] for r in rust_found.get("results", [])]
            )
            # And with enforcement on and no loopback asserted, both must withhold it.
            # NOTE: `py` was constructed without the flag and caches its config, so it would not
            # enforce; Rust re-reads per request and would. Using the enforcing Python instance
            # is what makes this a comparison of engines rather than of caching policies.
            py_held = py_export_engine.search("shard replicated", scope="all", provider="ollama", limit=5)
            rust_held = rust({"op": "search", "query": "shard replicated", "scope": "all",
                              "provider": "ollama", "limit": 5})
            checks += 1
            mismatches += int([r["source_ref"] for r in py_held.get("results", [])] !=
                              [r["source_ref"] for r in rust_held.get("results", [])])
            # Both engines must leave a sandboxed payload untouched by the vault sweep.
            py.index(force=True)
            rust({"op": "index", "force": True})
            after_py = py.search("shard replicated", scope="all", provider="ollama", limit=5, loopback=True)
            after_rust = rust({"op": "search", "query": "shard replicated", "scope": "all",
                               "provider": "ollama", "limit": 5, "loopback": True})
            checks += 1
            mismatches += int(
                py_sb.get("handle") not in [r["source_ref"] for r in after_py.get("results", [])] or
                rust_sb.get("handle") not in [r["source_ref"] for r in after_rust.get("results", [])]
            )
            # A context packet must be bounded identically in both engines, including when
            # the caller omits the budget. Rust previously returned an unbounded packet in
            # that case (24,149 chars vs Python's 4,803) and nothing caught it.
            # loopback asserted on both sides: this check is about the budget contract, not
            # about export policy, and by this point in the run enforcement is switched on.
            py_ctx = py.context(None, "retention", "ollama", 1200, None, True)
            rust_ctx = rust({"op": "context", "query": "retention", "provider": "ollama",
                             "loopback": True})
            checks += 1
            mismatches += int(py_ctx.get("char_budget") != rust_ctx.get("char_budget") or
                              (rust_ctx.get("char_budget") is None) or
                              abs(len(py_ctx.get("context", "")) - len(rust_ctx.get("context", ""))) > 40)
            checks += 1
            # Both engines must leave a config file the other can parse. The Rust writer once
            # emitted a literal backslash-n, so a fresh state initialized once and then failed
            # to start on every later run.
            def config_keys(path):
                try:
                    loaded = json.loads(path.read_text(encoding="utf-8"))
                except Exception:
                    return None
                return sorted(loaded) if isinstance(loaded, dict) else None

            py_cfg = config_keys(py_home / "config.json")
            rust_cfg = config_keys(rust_home / "config.json")
            checks += 1
            mismatches += int(py_cfg is None or rust_cfg is None or py_cfg != rust_cfg)
            checks += 1
            mismatches += int(py_home.stat().st_mode & 0o777 != 0o700 or
                              rust_home.stat().st_mode & 0o777 != 0o700 or
                              (rust_home / "config.json").stat().st_mode & 0o777 != 0o600 or
                              (rust_home / "index.sqlite3").stat().st_mode & 0o777 != 0o600)
            # Credential exposure alarms must agree across engines. The shared vault already carries a
            # credential note (`projects/sensitive.md`), so both engines see the same input and
            # the same FINGERPRINT and alarm_id must come out - or the owner is told two
            # different stories about which key to rotate depending on which engine ran.
            # `detected_at` is wall clock from two separate runs, so only its FORMAT is compared.
            py_alarms = py.alarms()["alarms"]
            rust_alarms = rust({"op": "alarms"})["alarms"]
            checks += 1
            mismatches += int(len(py_alarms) != 1 or len(rust_alarms) != 1)
            if py_alarms and rust_alarms:
                for field in ("alarm_id", "fingerprint", "secret_kind", "relpath"):
                    checks += 1
                    mismatches += int(py_alarms[0][field] != rust_alarms[0][field])
                checks += 1
                mismatches += int(len(py_alarms[0]["detected_at"]) != len(rust_alarms[0]["detected_at"])
                                  or not py_alarms[0]["detected_at"].endswith("+00:00")
                                  or not rust_alarms[0]["detected_at"].endswith("+00:00"))
                # The reminder must name the note but never carry the value, on either engine.
                for payload in (py_alarms, rust_alarms):
                    checks += 1
                    mismatches += int(candidate in json.dumps(payload))
            report = {"ok": mismatches == 0, "parity_checks": checks, "mismatches": mismatches,
                      "python_config_keys": len(py_cfg) if py_cfg else 0,
                      "rust_config_keys": len(rust_cfg) if rust_cfg else 0,
                      "export_policy_enforced": True,
                      "python_documents": py_index.get("documents"), "rust_documents": rust_index.get("documents"),
                      "rust_excluded_sensitive": rust_index.get("excluded_sensitive"),
                      "python_state_mode": oct(py_home.stat().st_mode & 0o777),
                      "rust_state_mode": oct(rust_home.stat().st_mode & 0o777),
                      "rust_config_mode": oct((rust_home / "config.json").stat().st_mode & 0o777),
                      "rust_index_mode": oct((rust_home / "index.sqlite3").stat().st_mode & 0o777)}
            print(json.dumps(report, separators=(",", ":")))
        finally:
            proc.stdin.close()
            proc.wait(timeout=5)
    return 0 if mismatches == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
