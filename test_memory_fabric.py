from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
import unittest
from contextlib import closing
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from memory_fabric import (SANDBOX_HEAD_CHARS, MemoryFabric, classify_storage_error,
                             dispatch_request, main)


class MemoryFabricTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        base = Path(self.temp.name)
        self.vault = base / "vault"
        self.home = base / "state"
        (self.vault / "projects").mkdir(parents=True)
        (self.vault / "pi-harness").mkdir()
        (self.vault / "company").mkdir()
        (self.vault / "Apple Notes").mkdir()
        (self.vault / "projects" / "atlas.md").write_text(
            "---\nstatus: current\nclassification: local_only\nvalid_from: 2024-01-01\n---\n\n# Atlas project\n\n## Decisions\n\nThe local retrieval service uses SQLite FTS5 and preserves source provenance.\n\n## Log\n\n- First entry.\n", encoding="utf-8")
        (self.vault / "projects" / "atlas").mkdir()
        (self.vault / "projects" / "atlas" / "legacy.md").write_text(
            "---\nstatus: superseded\nvalid_from: 2020-01-01\nvalid_to: 2024-01-01\nsuperseded_by: projects/atlas.md\n---\n\n# Legacy Atlas retention\n\nThe historic retention snapshots used the retired vault format.\n", encoding="utf-8")
        (self.vault / "projects" / "atlas" / "current-policy.md").write_text(
            "---\nstatus: current\nvalid_from: 2022-01-01\n---\n\n# Current Atlas policy\n\nCurrent retention snapshots replaced the old archive rules.\n", encoding="utf-8")
        (self.vault / "pi-harness" / "cache.md").write_text(
            "# Prompt cache behavior\n\n## Cache\n\nChanging an early prompt section can invalidate the cached conversation prefix. Append context to preserve reuse.\n", encoding="utf-8")
        (self.vault / "company" / "policy.md").write_text(
            "# Company privacy policy\n\n## Rules\n\nDurable memory changes require human review and approval.\n", encoding="utf-8")
        # A lookalike fixture proves the scanner skips the prohibited directory by name;
        # no real Apple Notes location is accessed.
        (self.vault / "Apple Notes" / "not-for-index.md").write_text("PRIVATE SENTINEL\n", encoding="utf-8")
        outside = base / "outside.md"
        outside.write_text("# Outside\n\nSecret symlink target.\n", encoding="utf-8")
        try:
            (self.vault / "projects" / "linked.md").symlink_to(outside)
        except OSError:
            pass
        self.fabric = MemoryFabric(home=self.home, vault=self.vault)
        self.result = self.fabric.index()

    def tearDown(self):
        self.temp.cleanup()

    def test_explicit_allowlist_company_project_and_system_scopes(self):
        self.assertTrue(self.result["ok"])
        with closing(self.fabric._connect()) as con:
            rows = dict(con.execute("SELECT relpath,scope FROM documents"))
        self.assertEqual(rows["projects/atlas.md"], "project:atlas")
        self.assertEqual(rows["company/policy.md"], "company")
        self.assertEqual(rows["pi-harness/cache.md"], "system:pi-harness")
        self.assertNotIn("Apple Notes/not-for-index.md", rows)
        self.assertNotIn("projects/linked.md", rows)
        self.assertEqual(self.home.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.home / "config.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.fabric.db_path.stat().st_mode & 0o777, 0o600)

    def test_scope_isolation_and_search_abstention(self):
        system = self.fabric.search("prompt cache early section", scope="system:pi-harness", provider="ollama")
        company = self.fabric.search("prompt cache early section", scope="company", provider="ollama")
        no_hit = self.fabric.search("mars weather orbital telescope", provider="ollama")
        self.assertEqual([r["source_ref"] for r in system["results"]], ["pi-harness/cache.md"])
        self.assertEqual(company["results"], [])
        self.assertIsNotNone(no_hit["abstention"])

    def test_common_terms_answer_with_a_label_instead_of_silence(self):
        """A term present in every note cannot discriminate. The engine must answer over the
        query's own tokens and label the result, reserving abstention for terms the index lacks."""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            for i in range(4):
                (vault / "projects" / f"n{i}.md").write_text(
                    f"# Note {i}\n\nalpha shared topic body number {i}\n", encoding="utf-8")
            fabric = MemoryFabric(home=base / "state", vault=vault)
            self.assertTrue(fabric.index()["ok"])

            common = fabric.search("alpha shared topic", provider="ollama")
            self.assertTrue(common["ok"])
            self.assertTrue(common["low_discrimination"], common)
            self.assertGreater(common["shown"], 0)
            self.assertGreaterEqual(common["total_matches"], common["shown"])
            self.assertIsNone(common["abstention"])

            absent = fabric.search("zirconium quasars", provider="ollama")
            self.assertEqual(absent["shown"], 0)
            self.assertEqual(absent["total_matches"], 0)
            self.assertFalse(absent["low_discrimination"])
            self.assertEqual(absent["abstention"], "no indexed source contains these terms")

    def test_export_policy_gates_notes_for_non_loopback_callers_when_enforced(self):
        """Every note is deny_remote by default, so enforcement must withhold everything from a
        non-loopback caller and report how many were held back - and must not withhold from a
        loopback one. A missing `loopback` assertion fails closed."""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            for i in range(3):
                (vault / "projects" / f"n{i}.md").write_text(
                    f"# Note {i}\n\nalpha shared topic body number {i}\n", encoding="utf-8")
            # enforce_export_policy is opt-in: turning it on overrides the operator's
            # blanket read exemption, so it must be an explicit decision, not a default.
            fabric = MemoryFabric(home=base / "state", vault=vault,
                                   config={"enforce_export_policy": True})
            self.assertTrue(fabric.index()["ok"])

            remote = fabric.search("alpha shared topic", provider="llama.cpp")
            self.assertTrue(remote["ok"])
            self.assertEqual(remote["results"], [], "non-loopback caller saw a deny_remote note")
            self.assertGreater(remote["withheld_export"], 0, "withheld count must be reported")

            missing_assertion = fabric.search("alpha shared topic", provider="llama.cpp")
            self.assertEqual(missing_assertion["results"], [], "missing loopback must fail closed")

            local = fabric.search("alpha shared topic", provider="llama.cpp", loopback=True)
            self.assertEqual(len(local["results"]), 3, "loopback caller must not be withheld")
            self.assertEqual(local["withheld_export"], 0)

    def test_busy_index_is_reported_as_transient_not_as_corruption(self):
        """A locked index must say so, so retrying is possible: 'database is locked' on its own
        says nothing about whether a retry will help, and a malformed index must not be
        dressed up as transient contention."""
        locked = classify_storage_error(sqlite3.OperationalError("database is locked"))
        self.assertIn("busy", locked.lower())
        self.assertIn("retry shortly", locked)
        self.assertIn("database is locked", locked, "the underlying detail must survive")

        self.assertEqual(classify_storage_error(sqlite3.OperationalError("no such table: documents")),
                         "no such table: documents")
        self.assertEqual(classify_storage_error(sqlite3.OperationalError(
            "database disk image is malformed")), "database disk image is malformed")

        # And end to end: hold an exclusive lock and confirm the CLI reports it as busy.
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            (vault / "projects" / "n.md").write_text("# N\n\nalpha beta\n", encoding="utf-8")
            fabric = MemoryFabric(home=base / "state", vault=vault)
            self.assertTrue(fabric.index()["ok"])
            with closing(fabric._connect()) as con:
                con.execute("PRAGMA journal_mode=WAL")
                con.execute("BEGIN EXCLUSIVE")
                try:
                    with self.assertRaises(sqlite3.OperationalError):
                        fabric.index(force=True)
                finally:
                    con.execute("ROLLBACK")
            self.assertTrue(fabric.index(force=True)["ok"], "retry after release must succeed")

    def test_status_reports_the_export_policy_knob(self):
        """Both the scrub and the export gate change what a session receives. If the knob is
        invisible, an operator cannot tell why their remote provider suddenly gets nothing."""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            (vault / "projects" / "n.md").write_text("# N\n\nalpha\n", encoding="utf-8")
            for cfg in ({}, {"enforce_export_policy": True}):
                home = base / ("s-on" if cfg else "s-off")
                fabric = MemoryFabric(home=home, vault=vault, config=cfg)
                fabric.index()
                self.assertEqual(fabric.status().get("enforce_export_policy"), bool(cfg))

    def test_export_policy_default_is_off_and_stays_reported(self):
        """Issue #8 decision, recorded as a test so it cannot drift silently.

        `enforce_export_policy` defaults to OFF and that is a deliberate operator decision for
        1.0, not an oversight. Reason, measured on this machine: every indexed note carries
        `deny_remote`, so turning enforcement on hands an exempted provider NOTHING - it does
        not tighten a boundary, it removes the operator's blanket read exemption wholesale.
        Turning it on properly requires a per-note `allow_remote` marking convention, and
        which notes may leave this machine is the operator's call, never an agent's.

        So the default must stay off, must be visible in `status`, and must not be flipped by
        an unrelated code change.
        """
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            (vault / "projects" / "n.md").write_text("# N\n\nalpha\n", encoding="utf-8")

            # No config at all: the default must be off.
            default_home = base / "default"
            MemoryFabric(home=default_home, vault=vault).index()
            self.assertFalse(MemoryFabric(home=default_home, vault=vault).status()
                             .get("enforce_export_policy"),
                             "enforce_export_policy must default to OFF")

            # Explicitly off is still off - the switch only ever turns it ON.
            off = MemoryFabric(home=base / "off", vault=vault,
                               config={"enforce_export_policy": False})
            off.index()
            self.assertFalse(off.status().get("enforce_export_policy"))

            # And the documented consequence: with it off, an exempted provider still reads.
            # `local_providers` is the engine's own allowlist; an exempted provider must be
            # listed there or it is refused before the export gate is ever reached.
            off2 = MemoryFabric(home=base / "off2", vault=vault,
                                config={"local_providers": ["opencode"],
                                        "enforce_export_policy": False})
            off2.index()
            remote = off2.search("alpha", provider="opencode")
            self.assertTrue(remote["ok"])
            self.assertEqual(remote.get("withheld_export"), 0,
                             "enforcement off means the read exemption stands")

            # With it on, the same caller gets nothing - the reason the default stays off.
            on = MemoryFabric(home=base / "on", vault=vault,
                              config={"local_providers": ["opencode"],
                                      "enforce_export_policy": True})
            on.index()
            blocked = on.search("alpha", provider="opencode")
            self.assertEqual(blocked["results"], [], "enforcement on withholds deny_remote notes")
            self.assertGreater(blocked["withheld_export"], 0)

    def test_sandbox_keeps_raw_bytes_out_of_context_and_retrieves_deliberately(self):
        """A large payload should cost a bounded slice of context, with the rest stored outside
        the canonical memory and reached only through search or read."""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            (vault / "projects" / "n.md").write_text("# N\n\nalpha beta\n", encoding="utf-8")
            fabric = MemoryFabric(home=base / "state", vault=vault)
            self.assertTrue(fabric.index()["ok"])
            payload = "shard three replicated log lines\n" * 400  # ~13 KB of tool output
            result = fabric.sandbox("big tool output", payload, provider="llama.cpp")
            self.assertTrue(result["ok"], result)
            self.assertEqual(result["chars"], len(payload))
            self.assertLessEqual(len(result["head"]), SANDBOX_HEAD_CHARS)
            self.assertEqual(result["scope"], "episodic")
            handle = result["handle"]
            # Raw bytes on disk, outside the canonical memory, private.
            raw = (base / "state" / "sandbox" / handle.split("/", 1)[1]).read_text(encoding="utf-8")
            self.assertEqual(raw, payload)
            self.assertEqual((base / "state" / "sandbox" / handle.split("/", 1)[1]).stat().st_mode & 0o777, 0o600)
            self.assertNotIn(handle, {str(p.relative_to(vault)) for p in vault.rglob("*.md")},
                             "sandbox must never write into the canonical memory")
            # Retrievable, and it survives a reindex rather than being swept as stale.
            hits = fabric.search("shard replicated", scope="all", provider="llama.cpp")
            self.assertIn(handle, [r["source_ref"] for r in hits["results"]])
            fabric.index(force=True)
            after = fabric.search("shard replicated", scope="all", provider="llama.cpp")
            self.assertIn(handle, [r["source_ref"] for r in after["results"]],
                         "sandboxed payload was dropped by the stale sweep")

    def test_sandbox_refuses_secrets_empty_and_unknown_providers(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            vault = base / "vault"
            (vault / "projects").mkdir(parents=True)
            (vault / "projects" / "n.md").write_text("# N\n\nalpha\n", encoding="utf-8")
            fabric = MemoryFabric(home=base / "state", vault=vault)
            fabric.index()
            secret = fabric.sandbox("x", "SUPABASE_SERVICE_ROLE_KEY=" + "CANDIDATE" + "X" * 24,
                                    provider="llama.cpp")
            self.assertFalse(secret["ok"])
            self.assertEqual(secret.get("error", "").split(" ")[0], "sandbox")
            # Nothing may have been written for a rejected payload.
            sandbox_files = list((base / "state" / "sandbox").glob("*.md")) if (base / "state" / "sandbox").exists() else []
            self.assertEqual(sandbox_files, [], "a rejected payload must leave no file")
            self.assertFalse(fabric.sandbox("x", "   ", provider="llama.cpp")["ok"])
            blocked = fabric.sandbox("x", "literature", provider="google")
            self.assertFalse(blocked["ok"])
            self.assertIn("blocked", blocked)

    def test_as_of_filters_superseded_sources_and_preserves_provenance(self):
        current = self.fabric.search("legacy retention snapshots", scope="project:atlas", provider="ollama")
        historic = self.fabric.search("legacy retention snapshots", scope="project:atlas", provider="ollama", as_of="2023-06-01")
        too_early = self.fabric.search("legacy retention snapshots", scope="project:atlas", provider="ollama", as_of="2019-01-01")
        invalid = self.fabric.search("legacy retention snapshots", provider="ollama", as_of="not-a-date")
        self.assertEqual([r["source_ref"] for r in current["results"]], ["projects/atlas/current-policy.md"])
        self.assertIn("No as_of date supplied", current["temporal_warning"])
        historic_refs = {r["source_ref"] for r in historic["results"]}
        self.assertEqual(historic_refs, {"projects/atlas/legacy.md", "projects/atlas/current-policy.md"})
        old = next(r for r in historic["results"] if r["source_ref"].endswith("legacy.md"))
        self.assertEqual(old["source_status"], "superseded")
        self.assertEqual(old["superseded_by"], "projects/atlas.md")
        self.assertEqual(old["export_policy"], "deny_remote")
        self.assertIn("Overlapping source lifecycle", historic["temporal_warning"])
        self.assertEqual(too_early["results"], [])
        self.assertFalse(invalid["ok"])

    def test_remote_provider_cannot_receive_paths_or_content(self):
        search = self.fabric.search("retrieval provenance SQLite", provider="opencode-go")
        read = self.fabric.read("projects/atlas.md", provider="anthropic")
        self.assertEqual(set(search), {"ok", "blocked"})
        self.assertEqual(set(read), {"ok", "blocked"})
        self.assertNotIn("atlas", json.dumps(search).lower())
        self.assertNotIn("retrieval", json.dumps(search).lower())

    def test_context_is_project_scoped_bounded_and_local_only(self):
        blocked = self.fabric.context("atlas", "SQLite retrieval provenance", provider="anthropic")
        self.assertEqual(set(blocked), {"ok", "blocked"})
        packet = self.fabric.context("atlas", "SQLite retrieval provenance", provider="ollama", token_budget=100)
        self.assertTrue(packet["ok"])
        self.assertEqual(packet["project"], "atlas")
        self.assertLessEqual(packet["estimated_characters"], packet["char_budget"])
        self.assertIn("projects/atlas.md", packet["context"])
        self.assertIn("source_ref", packet["instruction"])

    def test_local_read_requires_index_and_checks_staleness(self):
        read = self.fabric.read("projects/atlas.md", provider="ollama", heading="Decisions")
        self.assertTrue(read["ok"])
        self.assertIn("SQLite FTS5", read["content"])
        (self.vault / "projects" / "atlas.md").write_text("# changed\n", encoding="utf-8")
        stale = self.fabric.read("projects/atlas.md", provider="ollama")
        self.assertTrue(stale["stale"])

    def test_safe_proposal_is_outside_vault_and_never_applies(self):
        before = (self.vault / "projects" / "atlas.md").read_bytes()
        result = self.fabric.propose("atlas", "projects/atlas.md", "Log", "- Candidate learning, pending owner review.",
                                     ["pi-harness/cache.md"], provider="opencode-go")
        self.assertTrue(result["ok"])
        self.assertNotIn("diff", result)
        after = (self.vault / "projects" / "atlas.md").read_bytes()
        self.assertEqual(before, after)
        stored = self.fabric.proposals_path / f"{result['proposal_id']}.json"
        self.assertTrue(stored.exists())
        self.assertEqual(self.fabric.proposals_path.stat().st_mode & 0o777, 0o700)
        self.assertEqual(stored.stat().st_mode & 0o777, 0o600)
        record = json.loads(stored.read_text())
        self.assertEqual(record["classification"], "local_only")
        self.assertEqual(record["status"], "proposed_review_required")

    def test_proposal_secret_filter_and_path_policy(self):
        candidate = "CANDIDATE" + "X" * 24
        blocked = self.fabric.propose("atlas", "projects/atlas.md", "Log", f"SERVICE_ROLE_KEY={candidate}", provider="ollama")
        outside = self.fabric.propose("atlas", "STATUS.md", "Log", "Candidate.", provider="ollama")
        self.assertFalse(blocked["ok"])
        self.assertNotIn(candidate, json.dumps(blocked))
        self.assertFalse(outside["ok"])
        self.assertEqual(self.fabric.proposal_list()["proposals"], [])

    def test_index_secret_filter_excludes_whole_note_without_echoing(self):
        secret_note = self.vault / "projects" / "sensitive.md"
        candidate = "CANDIDATE" + "X" * 24
        secret_note.write_text(f"# Sensitive\n\nSUPABASE_SERVICE_ROLE_KEY={candidate}\n", encoding="utf-8")
        result = self.fabric.index()
        self.assertEqual(result["excluded_sensitive"], 1)
        with closing(self.fabric._connect()) as con:
            self.assertIsNone(con.execute("SELECT 1 FROM documents WHERE relpath='projects/sensitive.md'").fetchone())

    def test_secret_filter_catches_underscore_prefix_without_task_false_positive(self):
        candidate = "CANDIDATE" + "X" * 24
        service_role = self.fabric.propose("atlas", "projects/atlas.md", "Log",
                                           f"SUPABASE_SERVICE_ROLE_KEY={candidate}", provider="ollama")
        openai = self.fabric.propose("atlas", "projects/atlas.md", "Log",
                                     "sk-" + "proj-" + "A" * 25, provider="ollama")
        plain = self.fabric.propose("atlas", "projects/atlas.md", "Log",
                                    "Task identifiers can contain task- words without credentials.", provider="ollama")
        placeholder = self.fabric.propose("atlas", "projects/atlas.md", "Log",
                                          "SUPABASE_SERVICE_ROLE_KEY=YOUR_API_KEY_HERE", provider="ollama")
        self.assertFalse(service_role["ok"])
        self.assertFalse(openai["ok"])
        self.assertTrue(plain["ok"])
        self.assertTrue(placeholder["ok"])
        self.assertNotIn(candidate, json.dumps(service_role))

    def test_read_rejects_traversal_and_replaced_symlink(self):
        traversal = self.fabric.read("../outside.md", provider="ollama")
        self.assertFalse(traversal["ok"])
        target = self.vault / "projects" / "atlas.md"
        target.unlink()
        outside = self.vault.parent / "outside.md"
        target.symlink_to(outside)
        blocked = self.fabric.read("projects/atlas.md", provider="ollama")
        self.assertFalse(blocked["ok"])

    def test_index_only_changes_generated_index(self):
        source = self.vault / "projects" / "atlas.md"
        old = source.read_bytes()
        second = self.fabric.index()
        self.assertEqual(second["updated"], 0)
        self.assertEqual(source.read_bytes(), old)
        source.unlink()
        third = self.fabric.index()
        self.assertEqual(third["removed_from_index"], 1)
        self.assertFalse(source.exists())


if __name__ == "__main__":
    unittest.main()
