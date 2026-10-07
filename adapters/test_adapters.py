"""Each example adapter against a fake CLI on PATH: the lines it emits and what an ack does.

Run with: python3 -m unittest discover -q -s adapters
"""
import json
import os
import pathlib
import queue
import subprocess
import sys
import tempfile
import textwrap
import threading
import unittest

HERE = pathlib.Path(__file__).resolve().parent


class Adapter:
    def __init__(self, name, uri, fakes):
        self.bin = pathlib.Path(tempfile.mkdtemp())
        self.log = self.bin / "calls.jsonl"
        for cli, body in fakes.items():
            path = self.bin / cli
            path.write_text(
                f"#!{sys.executable}\nimport json, sys, time\n"
                f"open({str(self.log)!r}, 'a').write(json.dumps(sys.argv[1:]) + '\\n')\n"
                + textwrap.dedent(body)
            )
            path.chmod(0o755)
        env = {**os.environ, "PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}"}
        self.proc = subprocess.Popen(
            [str(HERE / name), uri], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env
        )
        self.lines = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            self.lines.put(json.loads(line))

    def next(self):
        return self.lines.get(timeout=30)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)
        self.proc.stdout.close()


class ExampleAdapterTests(unittest.TestCase):
    def test_github_emits_each_finished_run_once_and_skips_one_in_progress(self):
        runs = {"total_count": 2, "workflow_runs": [
            {"id": 9120, "name": "CI", "head_branch": "main", "head_sha": "4f1c0de", "status": "in_progress",
             "conclusion": None, "run_attempt": 1, "html_url": "https://github.com/o/r/actions/runs/9120"},
            {"id": 9119, "name": "CI", "head_branch": "main", "head_sha": "9ab77e1", "status": "completed",
             "conclusion": "failure", "run_attempt": 2, "html_url": "https://github.com/o/r/actions/runs/9119"},
        ]}
        adapter = Adapter("yi-adapter-github", "github://o/r?every=1s", {"gh": f"print(json.dumps({runs!r}))\n"})
        try:
            line = adapter.next()
            self.assertEqual(line["id"], "9119-2")
            self.assertEqual(line["data"]["conclusion"], "failure")
            self.assertEqual(line["data"]["sha"], "9ab77e1")
            with self.assertRaises(queue.Empty):
                adapter.lines.get(timeout=1.5)
        finally:
            adapter.close()
        self.assertEqual(adapter.calls()[0], ["api", "repos/o/r/actions/runs?per_page=20"])

    def test_forgejo_emits_a_run_once_every_job_ended_and_names_its_failure(self):
        tasks = {"total_count": 3, "workflow_runs": [
            {"id": 811, "name": "gate", "status": "failure", "event": "pull_request", "run_number": 41,
             "head_sha": "c0ffee1", "head_branch": "jack/x", "display_title": "Add x"},
            {"id": 812, "name": "title", "status": "success", "event": "pull_request", "run_number": 41,
             "head_sha": "c0ffee1", "head_branch": "jack/x", "display_title": "Add x"},
            {"id": 813, "name": "gate", "status": "running", "event": "push", "run_number": 42,
             "head_sha": "d00d", "head_branch": "main", "display_title": "Merge"},
        ]}
        adapter = Adapter(
            "yi-adapter-forgejo", "forgejo://git.example.invalid/apex/yi?every=1s",
            {"fgj": f"print(json.dumps({tasks!r}))\n"},
        )
        try:
            line = adapter.next()
            self.assertEqual(line["id"], "run-41")
            self.assertEqual(line["data"]["conclusion"], "failure")
            self.assertEqual(line["data"]["jobs"], {"gate": "failure", "title": "success"})
            with self.assertRaises(queue.Empty):
                adapter.lines.get(timeout=1.5)
        finally:
            adapter.close()
        self.assertEqual(
            adapter.calls()[0], ["api", "--hostname", "git.example.invalid", "repos/apex/yi/actions/tasks?limit=50"]
        )

    def test_forgejo_pulls_emits_each_open_head_once_per_sha(self):
        pulls = [
            {"number": 733, "title": "WIP: Open PRs as drafts", "head": {"sha": "0813a8ef5b2c77d1", "ref": "claude/x"}},
            {"number": 700, "title": "Search past sessions", "head": {"sha": "9ab77e1c0ffee000", "ref": "jack/y"}},
        ]
        adapter = Adapter(
            "yi-adapter-forgejo", "forgejo://git.example.invalid/apex/yi/pulls?every=1s",
            {"fgj": f"print(json.dumps({pulls!r}))\n"},
        )
        try:
            first, second = adapter.next(), adapter.next()
            self.assertEqual(first["id"], "pr-733-0813a8ef5b2c")
            self.assertEqual(first["data"], {"pr": 733, "sha": "0813a8ef5b2c77d1", "branch": "claude/x",
                                             "title": "WIP: Open PRs as drafts", "draft": True})
            self.assertFalse(second["data"]["draft"])
            with self.assertRaises(queue.Empty):
                adapter.lines.get(timeout=1.5)
        finally:
            adapter.close()
        self.assertEqual(
            adapter.calls()[0], ["api", "--hostname", "git.example.invalid", "repos/apex/yi/pulls?state=open&limit=50"]
        )


if __name__ == "__main__":
    unittest.main()
