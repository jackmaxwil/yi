import contextlib
import io
import json
import os
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock

import memory_inject


class Reply:
    def __init__(self, payload):
        self.payload = payload

    def __enter__(self):
        return io.BytesIO(json.dumps(self.payload).encode())

    def __exit__(self, *_args):
        return False


def answer(cost=0.25):
    payload = {
        "choices": [{"message": {"content": json.dumps({"relevant": [{"name": "lane-pool", "why": "matching rule"}]})}}],
        "usage": {},
    }
    if cost is not None:
        payload["usage"]["cost"] = cost
    return payload


class LabelTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.fixture = Path(self.temp.name)
        store = self.fixture / "home" / ".yi" / "memory"
        store.mkdir(parents=True)
        (store / "lane-pool.md").write_text(
            "---\nname: lane-pool\ndescription: lane rule\ntype: project\n---\nbody\n"
        )
        self.labels = self.fixture / "labels.json"
        self.labels.write_text(json.dumps({
            "suite": "test",
            "labeling": {"by": "hand"},
            "cases": [
                {"id": "one", "prompt": "lane", "relevant": {"lane-pool": "hand"}},
                {"id": "two", "prompt": "lane again", "relevant": {"lane-pool": "hand"}},
            ],
        }, indent=2) + "\n")
        self.original = self.labels.read_bytes()
        self.patches = [
            mock.patch.object(memory_inject, "FIXTURE", self.fixture),
            mock.patch.object(memory_inject, "LABELS", self.labels),
            mock.patch.object(memory_inject, "BEFORE", self.fixture / "before" / "block.txt"),
            mock.patch.dict(os.environ, {"OPENROUTER_API_KEY": "test"}),
        ]
        for patch in self.patches:
            patch.start()
        self.args = Namespace(model="openrouter/test-judge", cap_usd=1.0)

    def tearDown(self):
        for patch in reversed(self.patches):
            patch.stop()
        self.temp.cleanup()

    def test_label_writes_a_separate_fully_judged_arm(self):
        with mock.patch.object(memory_inject.urllib.request, "urlopen", side_effect=[Reply(answer(1.0))]) as post:
            self.assertEqual(memory_inject.label(self.args), 0)
        self.assertEqual(post.call_count, 1)
        self.assertEqual(self.labels.read_bytes(), self.original)
        judged = json.loads((self.fixture / "labels-judge.json").read_text())
        self.assertEqual(len(judged["cases"]), 1)
        self.assertEqual(judged["cases"][0]["judge"], "openrouter/test-judge")
        self.assertEqual(judged["cases"][0]["relevant"], {"lane-pool": "matching rule"})
        self.assertEqual(judged["labeling"]["by"], "judge openrouter/test-judge")

    def test_label_refuses_a_reply_whose_cost_cannot_be_counted(self):
        complaint = io.StringIO()
        with mock.patch.object(memory_inject.urllib.request, "urlopen", return_value=Reply(answer(None))):
            with contextlib.redirect_stderr(complaint):
                self.assertEqual(memory_inject.label(self.args), 1)
        self.assertEqual(
            complaint.getvalue(),
            "FAIL label: refused: the reply carried no usage.cost, so --cap-usd cannot count it\n",
        )
        self.assertEqual(self.labels.read_bytes(), self.original)
        self.assertFalse((self.fixture / "labels-judge.json").exists())

    def test_replay_and_check_consume_the_selected_judged_labels(self):
        judged = self.fixture / "labels-judge.json"
        judged.write_text(json.dumps({
            "suite": "judge arm",
            "labeling": {"by": "judge openrouter/test-judge"},
            "cases": [
                {"id": "one", "prompt": "lane", "relevant": {"lane-pool": "matching rule"}},
            ],
        }))
        before = self.fixture / "before"
        before.mkdir()
        block = "- [lane-pool](lane-pool.md) — lane rule\n## lane-pool — lane rule\nbody\n"
        (before / "block.txt").write_text(block)
        blocks = self.fixture / "after.jsonl"
        blocks.write_text(json.dumps({"id": "one", "block": block}) + "\n")
        with contextlib.redirect_stdout(io.StringIO()):
            replayed = memory_inject.main([
                "replay", "--labels", str(judged), "--blocks", str(blocks),
            ])
            checked = memory_inject.main(["check", "--labels", str(judged)])
        self.assertEqual((replayed, checked), (0, 0))


if __name__ == "__main__":
    unittest.main()
