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
from memory_fabric import MemoryFabric  # noqa: E402

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
            report = {"ok": mismatches == 0, "parity_checks": checks, "mismatches": mismatches,
                      "python_config_keys": len(py_cfg) if py_cfg else 0,
                      "rust_config_keys": len(rust_cfg) if rust_cfg else 0,
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
