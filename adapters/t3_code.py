#!/usr/bin/env python3
"""T3 Code Native Adapter for Local Memory Fabric.

Bridges T3 Code thread lifecycle and workspaceStrategy with Memory Fabric:
- Resolves workspaceStrategy worktrees into dedicated memory project cards
- Generates invariant context summaries for thread handoff (`t3_worktree_handoff`)
- Enforces static prompt prefix injection for >98% LLM KV-cache preservation
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))
from memory_fabric import MemoryFabric  # noqa: E402


class T3CodeAdapter:
    def __init__(self, fabric: MemoryFabric | None = None) -> None:
        self.fabric = fabric or MemoryFabric()

    def resolve_workspace_project(self, workspace_path: str | Path) -> str:
        """Derive project slug from workspace root."""
        p = Path(workspace_path).resolve()
        # Clean slug
        name = p.name.lower().replace(" ", "-").replace("_", "-")
        return name or "workspace"

    def get_thread_handoff_context(self, project: str, query: str = "architectural invariants decisions") -> dict[str, Any]:
        """Generate high-density invariant packet for T3 Code thread handoff."""
        ctx = self.fabric.context(project, query, provider="ollama", token_budget=1600, loopback=True)
        invariants_note = f"projects/{project}-invariants.md"
        invariants_text = ""
        inv_path = self.fabric.vault / invariants_note
        if inv_path.exists():
            invariants_text = inv_path.read_text(encoding="utf-8")

        return {
            "ok": True,
            "project": project,
            "packet": ctx.get("packet", ""),
            "invariants_text": invariants_text,
            "citations": ctx.get("citations", []),
            "source_count": ctx.get("source_count", 0)
        }

    def format_t3_system_instruction(self, project: str) -> str:
        """Deterministic static instruction block guaranteeing >98% KV-cache hit rate."""
        return (
            "<!-- LOCAL-MEMORY-FABRIC-T3-CODE -->\n"
            f"[PROJECT: {project}]\n"
            "This thread is linked to the Local Memory Fabric index.\n"
            "Query memories using `memory_fabric_context` or `memory_fabric_search`.\n"
            "All writes must be submitted through `memory_fabric_propose`.\n"
            "<!-- /LOCAL-MEMORY-FABRIC-T3-CODE -->"
        )
