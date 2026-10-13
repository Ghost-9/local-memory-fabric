#!/usr/bin/env python3
"""Unit tests for Native Agent Adapter Matrix & MCP Server Surface."""
from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent
sys.path.insert(0, str(REPO))
from adapters.mcp import McpServer, coerce_bool, coerce_num, coerce_str, format_anchored_uri
from adapters.t3_code import T3CodeAdapter


class TestAdapterMatrix(unittest.TestCase):
    def test_parameter_coercion(self):
        # Number coercion
        self.assertEqual(coerce_num("1200"), 1200)
        self.assertEqual(coerce_num("15.5"), 15)
        self.assertEqual(coerce_num(None, 42), 42)
        self.assertEqual(coerce_num("not_a_number", 10), 10)

        # String coercion
        self.assertEqual(coerce_str(123), "123")
        self.assertEqual(coerce_str(None, "default"), "default")
        self.assertEqual(coerce_str("valid"), "valid")

        # Bool coercion
        self.assertTrue(coerce_bool("true"))
        self.assertTrue(coerce_bool("1"))
        self.assertTrue(coerce_bool("yes"))
        self.assertFalse(coerce_bool("false"))
        self.assertFalse(coerce_bool(None))

    def test_anchored_uri_format(self):
        vault = Path("/tmp/vault")
        uri = format_anchored_uri(vault, "projects/atlas.md", 25)
        self.assertTrue(uri.startswith("file:///"))
        self.assertTrue(uri.endswith("#L25"))

    def test_mcp_protocol_initialize_and_tools(self):
        server = McpServer()
        init_res = server.handle_request({"id": 1, "method": "initialize", "params": {}})
        self.assertEqual(init_res["id"], 1)
        self.assertEqual(init_res["result"]["serverInfo"]["name"], "local-memory-fabric-mcp")

        tools_res = server.handle_request({"id": 2, "method": "tools/list", "params": {}})
        tools = tools_res["result"]["tools"]
        tool_names = [t["name"] for t in tools]
        self.assertIn("memory_fabric_search", tool_names)
        self.assertIn("memory_fabric_context", tool_names)
        self.assertIn("memory_fabric_compact", tool_names)
        self.assertIn("memory_fabric_federate", tool_names)
        self.assertIn("memory_fabric_symbiosis", tool_names)
        self.assertIn("memory_fabric_fuzzy_sections", tool_names)

        # Check annotations
        for t in tools:
            self.assertIn("annotations", t)
            self.assertFalse(t["annotations"]["destructiveHint"])

    def test_mcp_prompts_kv_cache_prefix(self):
        server = McpServer()
        prompt_res = server.handle_request({"id": 3, "method": "prompts/get", "params": {"name": "memory_fabric_system_prefix"}})
        msg = prompt_res["result"]["messages"][0]["content"]["text"]
        self.assertIn("Local Memory Fabric", msg)
        self.assertNotIn("2026-", msg)  # No dynamic timestamps in static prefix

    def test_t3_code_adapter(self):
        t3 = T3CodeAdapter()
        slug = t3.resolve_workspace_project("/Users/batra/projects/My Cool App")
        self.assertEqual(slug, "my-cool-app")
        instruction = t3.format_t3_system_instruction("my-cool-app")
        self.assertIn("[PROJECT: my-cool-app]", instruction)
        self.assertIn("LOCAL-MEMORY-FABRIC-T3-CODE", instruction)



    def test_fsearch_symbiosis_and_fuzzy_sections(self):
        server = McpServer()
        # Test fuzzy score
        score = server.fabric._fuzzy_score("arch", "architecture-overview")
        self.assertGreater(score, 0.5)
        # Test path proximity
        prox = server.fabric._calc_path_proximity("rust/src/retrieval.rs", "rust/src/main.rs")
        self.assertGreater(prox, 0.5)

if __name__ == "__main__":
    unittest.main()
