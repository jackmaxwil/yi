"""The global harness store is saved through the host; nothing else is.

stdlib unittest; the host's `harness.save_global` is faked at `rlm._host_request_blocking`.
"""
from __future__ import annotations

import json
import os
import pathlib
import shutil
import tempfile
import unittest
from unittest import mock

import rlm
from rlm.harness import _state_cache


class GlobalSaveTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root, True)
        self.store = self.root / "global"
        env = {"RLM_GLOBAL_HARNESS_HOST": "1", "RLM_GLOBAL_HARNESS_STATE_DIR": str(self.store)}
        patches = [mock.patch.dict(os.environ, env), mock.patch.dict(_state_cache, clear=True)]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def test_a_global_state_elsewhere_never_replaces_the_real_store(self) -> None:
        sent: list[dict] = []
        with mock.patch.object(rlm, "_host_request_blocking", lambda _t, p: sent.append(p) or {}):
            other = self.root / "elsewhere"
            rlm.get_harness_state(state_dir=other, global_=True).create_memory("note", "x")
        self.assertEqual(sent, [], "the host writes only the real store, so it must not get this one")
        saved = json.loads((other / "harness_state.json").read_text(encoding="utf-8"))
        self.assertIn("note", saved["entries"]["memory"])

    def test_a_refused_save_does_not_ride_along_on_the_next(self) -> None:
        sent: list[dict] = []

        def host(_type: str, payload: dict) -> dict:
            sent.append(payload)
            if len(sent) == 1:
                raise RuntimeError("Permission denied: user said no")
            return {}

        with mock.patch.object(rlm, "_host_request_blocking", host):
            state = rlm.get_harness_state(global_=True)
            with self.assertRaises(RuntimeError):
                state.create_memory("refused", "x")
            self.assertIsNone(state.get("memory", "refused"))
            state.create_memory("approved", "y")
        self.assertEqual(sorted(sent[-1]["state"]["entries"]["memory"]), ["approved"])


if __name__ == "__main__":
    unittest.main()
