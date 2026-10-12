"""`yi.tools`: a cell calls the session's tools and can tell a refusal from a failure."""
from __future__ import annotations

import asyncio
import inspect
import unittest

import rlm
import yi
from yi import tools

CATALOG = {
    "tools": [
        {"name": "grep", "description": "Search file contents with a regex.",
         "parameters": {"type": "object", "required": ["pattern"],
                        "properties": {"pattern": {"type": "string"}, "path": {"type": "string"},
                                       "context": {"type": "integer"}, "def": {"type": "string"}}}},
        {"name": "edit", "description": "Apply a hashline patch.",
         "parameters": {"type": "object", "required": ["patch"],
                        "properties": {"patch": {"type": "string"}}}},
        {"name": "write", "description": "Write a whole file.",
         "parameters": {"type": "object", "required": ["path", "content"],
                        "properties": {"path": {"type": "string"}, "content": {"type": "string"}}}},
    ],
    "excluded": [{"name": "ipython", "why": "a cell holds the kernel for its whole run"}],
}


class Tools(unittest.TestCase):
    def setUp(self) -> None:
        self.sent: list[tuple[str, dict]] = []
        self.reply: dict = {"ok": True, "text": "[a.rs#1A2B]\n1:fn a() {}", "details": {"hits": 1}}
        self.real = (rlm.host_request, rlm._host_request_blocking)

        async def request(kind: str, payload: dict | None = None) -> dict:
            self.sent.append((kind, payload or {}))
            return self.reply

        rlm.host_request = request
        rlm._host_request_blocking = lambda kind, payload=None: CATALOG
        tools._catalog = None

    def tearDown(self) -> None:
        rlm.host_request, rlm._host_request_blocking = self.real
        tools._catalog = None

    def test_a_call_sends_the_name_and_the_arguments_it_was_given(self) -> None:
        result = asyncio.run(yi.tools.grep(pattern="fn a", path=None))
        self.assertEqual(self.sent, [("tool.call", {"name": "grep", "args": {"pattern": "fn a"},
                                                    "cell": None})])
        self.assertEqual(result.text, "[a.rs#1A2B]\n1:fn a() {}")
        self.assertEqual(repr(result), result.text, "a cell's last value shows the view")
        self.assertEqual(result.details["hits"], 1)

    def test_a_tool_argument_called_name_is_sent_as_an_argument(self) -> None:
        asyncio.run(tools.call("deploy", name="web"))
        self.assertEqual(self.sent[0][1]["args"], {"name": "web"})

    def test_each_failure_raises_what_a_program_can_catch_apart(self) -> None:
        cases = [
            ({"ok": False, "kind": "refused", "message": "Permission denied: outside the tree"},
             yi.Refused),
            ({"ok": False, "kind": "invalid_input", "message": "patch is required"}, tools.InvalidInput),
            ({"ok": False, "kind": "unknown_tool", "message": "no tool gerp",
              "suggestions": ["grep"]}, tools.UnknownTool),
            ({"ok": False, "kind": "tool_failure", "text": "stale tag", "details": {}},
             tools.ToolFailure),
        ]
        for reply, raised in cases:
            with self.subTest(reply["kind"]):
                self.reply = reply
                with self.assertRaises(raised) as caught:
                    asyncio.run(tools.call("edit", patch="x"))
                self.assertIn(reply.get("message") or reply.get("text"), str(caught.exception))
        self.assertNotIsInstance(caught.exception, yi.Refused, "a failure is not a refusal")
        self.assertEqual(caught.exception.result.text, "stale tag")

    def test_a_tool_reads_as_a_function_with_its_own_parameters(self) -> None:
        signature = inspect.signature(yi.tools.grep)
        self.assertEqual(list(signature.parameters), ["pattern", "path", "context"],
                         "grep's real schema names a `def` parameter; a keyword is passed by **{}")
        self.assertIs(signature.parameters["pattern"].default, inspect.Parameter.empty)
        self.assertIs(signature.parameters["context"].annotation, int)
        self.assertEqual(yi.tools.grep.__doc__, "Search file contents with a regex.")
        self.assertIn("grep", dir(yi.tools))

    def test_a_missing_or_excluded_tool_names_what_exists_or_why(self) -> None:
        with self.assertRaises(tools.UnknownTool) as caught:
            _ = yi.tools.gerp
        self.assertIn("edit, grep, write", str(caught.exception))
        with self.assertRaises(tools.UnknownTool) as caught:
            _ = yi.tools.ipython
        self.assertIn("holds the kernel", str(caught.exception))
        self.assertFalse(hasattr(yi.tools, "gerp"), "hasattr stays a plain question")

    def test_search_names_its_cut_and_the_call_that_shows_the_rest(self) -> None:
        page = tools.search("", limit=2)
        self.assertEqual([row["name"] for row in page["tools"]], ["edit", "grep"])
        self.assertEqual((page["matched"], page["shown"]), (3, 2))
        self.assertIn("2 of 3 shown at limit=2", page["more"])
        self.assertIn("offset=2", page["more"])
        self.assertNotIn("more", tools.search("", limit=3))
        self.assertIn("ipython", page["left_out"], "a cell's listing names what it leaves out")
        self.assertEqual([row["name"] for row in tools.search("file")["tools"]], ["grep", "write"])


if __name__ == "__main__":
    unittest.main()
