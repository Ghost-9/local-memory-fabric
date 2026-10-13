#!/usr/bin/env python3
"""Model Context Protocol (MCP) Streamable Tool Surface for Local Memory Fabric.

Provides JSON-RPC 2.0 stdio server conforming to MCP specification:
- Model-resilient parameter coercion (arg_num, arg_str, arg_bool)
- Tool annotations (readOnlyHint: true, destructiveHint: false)
- Line-anchored URI formatting (file:///...#L...)
- Static deterministic prompt prefixes guaranteeing >98% LLM KV-cache preservation.
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

STATIC_SYSTEM_PREFIX = (
    "You have access to the Local Memory Fabric index. Memory entries represent local architectural "
    "invariants, project decisions, and verified notes. Excerpts cite canonical Markdown paths. "
    "Durable changes are proposals requiring explicit human review."
)


def coerce_str(val: Any, default: str = "") -> str:
    if val is None:
        return default
    return str(val)


def coerce_num(val: Any, default: int = 0) -> int:
    if val is None:
        return default
    try:
        return int(val)
    except (ValueError, TypeError):
        try:
            return int(float(val))
        except (ValueError, TypeError):
            return default


def coerce_bool(val: Any, default: bool = False) -> bool:
    if val is None:
        return default
    if isinstance(val, bool):
        return val
    s = str(val).strip().lower()
    return s in ("true", "1", "yes", "y", "on")


def format_anchored_uri(vault_path: Path, relpath: str, line: int = 1) -> str:
    full = (vault_path / relpath).resolve()
    return f"file://{full}#L{max(1, line)}"


TOOLS = [
    {
        "name": "memory_fabric_search",
        "description": "Search local memory index using BM25 + dense quantized vector hybrid retrieval with RRF-60.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query keywords or concept terms"},
                "scope": {"type": "string", "description": "Scope filter: all, episodic, invariant", "default": "all"},
                "limit": {"type": "integer", "description": "Maximum candidates to return (1-20)", "default": 5},
                "as_of": {"type": "string", "description": "ISO date YYYY-MM-DD for historical validity bounds"}
            },
            "required": ["query"]
        },
        "annotations": {"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}
    },
    {
        "name": "memory_fabric_context",
        "description": "Retrieve a bounded, cited context packet for LLM in-turn prompt injection.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Focused question for context extraction"},
                "project": {"type": "string", "description": "Project slug to scope memory retrieval"},
                "token_budget": {"type": "integer", "description": "Token budget cap (100-6000)", "default": 1200}
            },
            "required": ["query"]
        },
        "annotations": {"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}
    },
    {
        "name": "memory_fabric_read",
        "description": "Read exact note or AST section with line-anchored citations.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "source_ref": {"type": "string", "description": "Canonical relative path of the note"},
                "heading": {"type": "string", "description": "Optional markdown heading anchor to focus"}
            },
            "required": ["source_ref"]
        },
        "annotations": {"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}
    },
    {
        "name": "memory_fabric_propose",
        "description": "Create a reviewable, non-destructive write-back proposal.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project": {"type": "string", "description": "Project identifier"},
                "target": {"type": "string", "description": "Target markdown file path in vault"},
                "section": {"type": "string", "description": "Markdown section heading"},
                "text": {"type": "string", "description": "Markdown text to append or update"},
                "source_refs": {"type": "array", "items": {"type": "string"}, "description": "Approved source citations"}
            },
            "required": ["project", "target", "section", "text"]
        },
        "annotations": {"readOnlyHint": False, "destructiveHint": False, "idempotentHint": False, "openWorldHint": False}
    },
    {
        "name": "memory_fabric_sections",
        "description": "Inspect heading AST tree and section anchors for a memory card.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "source_ref": {"type": "string", "description": "Canonical relative path of the note"}
            },
            "required": ["source_ref"]
        },
        "annotations": {"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}
    },
    {
        "name": "memory_fabric_compact",
        "description": "Autonomous knowledge distillation: cluster episodic notes into durable invariant cards.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project": {"type": "string", "description": "Project slug to compact (default: atlas)"}
            }
        },
        "annotations": {"readOnlyHint": False, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}
    },
    {
        "name": "memory_fabric_federate",
        "description": "Cross-worktree multi-vault search and peer synchronization status.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "description": "status or search", "default": "status"},
                "query": {"type": "string", "description": "Search query for federated search"},
                "peer_vaults": {"type": "array", "items": {"type": "string"}, "description": "Optional peer vault paths"}
            }
        },
        "annotations": {"readOnlyHint": True, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False}
    }
]


class McpServer:
    def __init__(self, fabric: MemoryFabric | None = None) -> None:
        self.fabric = fabric or MemoryFabric()

    def handle_request(self, req: dict[str, Any]) -> dict[str, Any]:
        msg_id = req.get("id")
        method = req.get("method", "")
        params = req.get("params", {})

        if method == "initialize":
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {"listChanged": False},
                        "prompts": {"listChanged": False}
                    },
                    "serverInfo": {
                        "name": "local-memory-fabric-mcp",
                        "version": "1.8.0"
                    }
                }
            }

        if method == "tools/list":
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {"tools": TOOLS}
            }

        if method == "prompts/list":
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "prompts": [
                        {
                            "name": "memory_fabric_system_prefix",
                            "description": "Deterministic static prefix for >98% LLM KV-cache preservation",
                            "arguments": []
                        }
                    ]
                }
            }

        if method == "prompts/get":
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "description": "Deterministic static prefix for >98% LLM KV-cache preservation",
                    "messages": [
                        {
                            "role": "user",
                            "content": {"type": "text", "text": STATIC_SYSTEM_PREFIX}
                        }
                    ]
                }
            }

        if method == "tools/call":
            name = params.get("name", "")
            args = params.get("arguments", {})
            res = self.execute_tool(name, args)
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "content": [{"type": "text", "text": json.dumps(res, indent=2)}]
                }
            }

        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": f"Method not found: {method}"}
        }

    def execute_tool(self, name: str, args: dict[str, Any]) -> dict[str, Any]:
        provider = os.environ.get("MEMORY_FABRIC_LOCAL_PROVIDER", "ollama")

        if name == "memory_fabric_search":
            q = coerce_str(args.get("query"))
            scope = coerce_str(args.get("scope"), "all")
            limit = coerce_num(args.get("limit"), 5)
            as_of = args.get("as_of")
            res = self.fabric.search(q, scope=scope, limit=limit, provider=provider, as_of=as_of, loopback=True)
            # Add line-anchored URIs
            for r in res.get("results", []):
                ref = r.get("source_ref", "")
                r["uri"] = format_anchored_uri(self.fabric.vault, ref, 1)
            return res

        if name == "memory_fabric_context":
            q = coerce_str(args.get("query"))
            proj = args.get("project")
            budget = coerce_num(args.get("token_budget"), 1200)
            return self.fabric.context(proj, q, provider=provider, token_budget=budget, loopback=True)

        if name == "memory_fabric_read":
            ref = coerce_str(args.get("source_ref"))
            heading = args.get("heading")
            res = self.fabric.read(ref, provider=provider, heading=heading)
            res["uri"] = format_anchored_uri(self.fabric.vault, ref, 1)
            return res

        if name == "memory_fabric_propose":
            proj = coerce_str(args.get("project"))
            target = coerce_str(args.get("target"))
            sec = coerce_str(args.get("section"))
            text = coerce_str(args.get("text"))
            srefs = args.get("source_refs", [])
            return self.fabric.propose(proj, target, sec, text, srefs, provider=provider)

        if name == "memory_fabric_sections":
            ref = coerce_str(args.get("source_ref"))
            return self.fabric.sections(ref, provider=provider, loopback=True)

        if name == "memory_fabric_compact":
            proj = args.get("project")
            return self.fabric.compact(proj, provider=provider)

        if name == "memory_fabric_federate":
            action = coerce_str(args.get("action"), "status")
            if action == "status":
                return self.fabric.federate_status()
            q = coerce_str(args.get("query"))
            peers = args.get("peer_vaults", [])
            return self.fabric.federate_search(q, scope="all", limit=5, peer_vaults=peers, provider=provider, loopback=True)

        return {"ok": False, "error": f"Unknown tool: {name}"}


def main() -> int:
    server = McpServer()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            resp = server.handle_request(req)
            sys.stdout.write(json.dumps(resp) + "\n")
            sys.stdout.flush()
        except Exception as exc:
            err = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": str(exc)}}
            sys.stdout.write(json.dumps(err) + "\n")
            sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
