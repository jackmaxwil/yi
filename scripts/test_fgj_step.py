#!/usr/bin/env python3
"""The `install fgj` step of the bot workflows starts without the internet when the job image carries fgj, and retries a download otherwise.
Incident: 2026-11-04 (5h40) and 2026-11-18 (4h40), every queue, fixer and review run died on `Could not resolve host: codeberg.org`."""
import hashlib
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/guardrails"))
from check_workflows import workflow_job, workflow_step, workflow_steps

FILES = ("queue", "autofix", "review")
GOOD = b"genuine fgj v0.5.0 stand-in\n"
SUM = hashlib.sha256(GOOD).hexdigest()
PINNED = "7a161b57355ba6216eb12d2d185dc8b47a94efec06116ef4f6efa5e5eea06b39"


def step(name, text=None):
    """The run script of the `install fgj` step in a workflow's only job, dedented."""
    text = text if text is not None else (ROOT / f".forgejo/workflows/{name}.yml").read_text()
    job = workflow_job(text, name)
    found = workflow_step(workflow_steps(job), "install fgj").splitlines()
    body = found[found.index("        run: |") + 1:]
    return "\n".join(line[10:] for line in body if line.startswith(" " * 10)) + "\n"


def run(script, present=None, curl_fails=0):
    """Run the step with /usr/local/bin and the pinned sum redirected into a sandbox; returns (rc, curl calls, installed bytes)."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        bin_, stubs = tmp / "bin", tmp / "stubs"
        bin_.mkdir(), stubs.mkdir()
        if present is not None:
            (bin_ / "fgj").write_bytes(present)
            (bin_ / "fgj").chmod(0o755)
        (stubs / "curl").write_text(
            "#!/bin/sh\n"
            "# models curl's retry: a failed connect (exit 6) is retried only under --retry N with --retry-all-errors\n"
            'tries=1; all=0\n'
            'while [ $# -gt 0 ]; do case "$1" in -o) out=$2;; --retry) tries=$(($2+1));; --retry-all-errors) all=1;; esac; shift; done\n'
            '[ "$all" = 1 ] || tries=1\n'
            "while [ $tries -gt 0 ]; do\n"
            f'  n=$(cat {tmp}/calls 2>/dev/null || echo 0); n=$((n+1)); echo $n > {tmp}/calls\n'
            f'  if [ "$n" -gt {curl_fails} ]; then printf "%s" "{GOOD.decode()}" > "$out"; exit 0; fi\n'
            "  tries=$((tries-1))\n"
            "done\n"
            "exit 6\n"
        )
        (stubs / "sha256sum").write_text(
            "#!/bin/sh\n"
            "# stand-in checksum: the real tool, with the pinned sum swapped for the stand-in's\n"
            f'sed "s/{PINNED}/{SUM}/" | exec "$REAL_SHA" "$@"\n'
        )
        for stub in stubs.iterdir():
            stub.chmod(0o755)
        real = subprocess.run(["sh", "-c", "command -v sha256sum || command -v gsha256sum"], capture_output=True, text=True).stdout.strip()
        script = script.replace("/usr/local/bin", str(bin_))
        env = {"PATH": f"{stubs}:{bin_}:/usr/bin:/bin", "REAL_SHA": real}
        made = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True, timeout=60)
        calls = int((tmp / "calls").read_text()) if (tmp / "calls").exists() else 0
        installed = (bin_ / "fgj").read_bytes() if (bin_ / "fgj").exists() else None
        return made.returncode, calls, installed, made.stderr


class InstallFgj(unittest.TestCase):
    def test_present_correct_binary_makes_no_network_call(self):
        for name in FILES:
            with self.subTest(name):
                rc, calls, installed, err = run(step(name), present=GOOD)
                self.assertEqual((rc, calls, installed), (0, 0, GOOD), err)

    def test_download_retries_past_two_failed_connects(self):
        for name in FILES:
            with self.subTest(name):
                rc, calls, installed, err = run(step(name), curl_fails=2)
                self.assertEqual((rc, calls, installed), (0, 3, GOOD), err)

    def test_wrong_binary_is_replaced_by_a_verified_download(self):
        """Regression guard only: the old step always downloaded, so it passes there too."""
        for name in FILES:
            with self.subTest(name):
                rc, calls, installed, err = run(step(name), present=b"some other fgj\n")
                self.assertEqual((rc, calls, installed), (0, 1, GOOD), err)

    def test_download_asks_curl_to_retry(self):
        for name in FILES:
            self.assertIn("--retry 5 --retry-all-errors --retry-delay 3 --connect-timeout 10", step(name))

    def test_one_pin_per_file_and_the_three_steps_are_identical(self):
        steps = [step(name) for name in FILES]
        self.assertEqual(len(set(steps)), 1)
        self.assertEqual(len(re.findall(PINNED, steps[0])), 1)


if __name__ == "__main__":
    unittest.main()
