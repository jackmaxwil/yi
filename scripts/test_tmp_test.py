#!/usr/bin/env python3
"""Prove test_tmp.sh honors TMPDIR and removes its private directory on exit and on a signal."""
import os
import pathlib
import re
import select
import shutil
import signal
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
WRAPPER = ROOT / "scripts/test_tmp.sh"
MARKER = "TEST_TMPDIR="

class TmpSandbox(unittest.TestCase):
    def setUp(self):
        self.parent = tempfile.TemporaryDirectory(prefix="tmp sandbox ")
        self.root = pathlib.Path(self.parent.name)
        self.env = {**os.environ, "TMPDIR": str(self.root)}

    def tearDown(self):
        self.parent.cleanup()

    def assert_private(self, path):
        self.assertEqual(path.parent, self.root)
        self.assertRegex(path.name, r"^yt\.")
        self.assertNotEqual(path, self.root)

    @unittest.skipIf(shutil.which("cargo-nextest") is None, "cargo-nextest is not installed")
    def test_nextest_uses_a_private_inherited_tmpdir_and_removes_it(self):
        run = subprocess.run(
            ["cargo", "nextest", "run", "-p", "yi-types", "--test", "integration", "--no-capture", "--",
             "--exact", "the_suite_wrapper_can_observe_tmpdir"],
            cwd=ROOT, env=self.env, capture_output=True, text=True, timeout=120, check=False)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        paths = re.findall(rf"^{re.escape(MARKER)}(.+)$", run.stdout, re.MULTILINE)
        self.assertEqual(len(paths), 1, run.stdout + run.stderr)
        private = pathlib.Path(paths[0])
        self.assert_private(private)
        self.assertFalse(private.exists(), "the wrapper left its private TMPDIR behind")

    def test_a_killed_wrapper_removes_its_private_inherited_tmpdir(self):
        command = ["bash", "-c", f'printf "{MARKER}%s\\n" "$TMPDIR"; exec sleep 30']
        process = subprocess.Popen(
            [str(WRAPPER), *command], cwd=ROOT, env=self.env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.addCleanup(process.stdout.close)
        self.addCleanup(process.stderr.close)
        try:
            ready, _, _ = select.select([process.stdout], [], [], 10)
            self.assertTrue(ready, "the wrapped process did not report its TMPDIR")
            line = process.stdout.readline()
            self.assertTrue(line.startswith(MARKER), line)
            private = pathlib.Path(line.removeprefix(MARKER).strip())
            self.assert_private(private)
            os.killpg(process.pid, signal.SIGTERM)
            _, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 143, stderr)
            self.assertFalse(private.exists(), "the EXIT trap left the killed process's TMPDIR behind")
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()


if __name__ == "__main__":
    unittest.main()
