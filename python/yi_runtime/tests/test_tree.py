"""The kernel's half of Tree: what it sends the host, and what it hands back.

stdlib unittest with a fake host_request, as test_rlm.py does. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import pathlib
import unittest
from unittest import mock

import rlm


class TreeTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self.sent: list[tuple[str, dict]] = []

        async def fake(kind: str, payload: dict | None = None) -> dict:
            self.sent.append((kind, dict(payload or {})))
            return self.answers.get(kind, {})

        self.answers: dict[str, dict] = {}
        patch = mock.patch.object(rlm, "host_request", fake)
        patch.start()
        self.addCleanup(patch.stop)

    async def test_fork_names_its_source_and_hands_back_a_tree_with_the_hosts_rows(self) -> None:
        self.answers["tree.fork"] = {"name": "fix", "next": "a row"}
        made = await rlm.tree().fork("fix", carry=[".env"])
        self.assertEqual(self.sent, [("tree.fork", {"from": "self", "name": "fix", "carry": [".env"]})])
        self.assertEqual((made.name, made.next), ("fix", "a row"))
        self.assertIn("a row", repr(made))

    async def test_an_unset_argument_is_not_sent_as_null(self) -> None:
        self.answers["tree.diff"] = {"files": [], "summary": "0 files differ"}
        reply = await rlm.tree("fix").diff()
        self.assertEqual(self.sent, [("tree.diff", {"tree": "fix"})])
        self.assertEqual(reply.summary, "0 files differ")

    async def test_merge_takes_trees_handles_and_names(self) -> None:
        self.answers["tree.merge"] = {"summary": "merged 3 of 3"}
        handle = rlm.RLMSpawnHandle(rlm_child_id="sub-1", name="kid", session_dir=pathlib.Path("/tmp"), model="faux/faux-1")
        await rlm.tree().merge(rlm.tree("a"), handle.tree, "c")
        self.assertEqual(self.sent, [("tree.merge", {"tree": "self", "others": ["a", "kid", "c"]})])

    async def test_a_tree_reaches_a_spawn_by_name(self) -> None:
        self.answers["rlm.run"] = {"child_id": "sub-1", "name": "kid", "session_dir": "/tmp", "model": "m"}
        try:
            await rlm.run("look", tree=rlm.tree("fix"))
        except Exception:  # a fake reply need not be a whole handle; the send is what is asked
            pass
        self.assertEqual(self.sent[0][1]["kwargs"]["tree"], "fix")

    def test_the_tree_verbs_are_on_the_public_surface(self) -> None:
        self.assertIn("tree", rlm.__all__)
        self.assertTrue(callable(rlm.tree))


if __name__ == "__main__":
    unittest.main()
