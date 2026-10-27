#!/usr/bin/env python3
"""cargo-deny, told apart from cargo-deny that could not run.
Incident: inside a read-only or offline sandbox the advisory database lock failed, deny exited 1,
and agents reported the whole guardrails lane unrunnable while every other gate was green. A
database that cannot be reached is a skip row; a finding (`error[...]`) always fails. CI sets
GITHUB_ACTIONS and keeps the skip off. The database path is cargo-deny's own: CARGO_HOME."""
import os
import re
import subprocess
import sys

UNREACHABLE = re.compile(r"advisory database|`cargo metadata` exited|failed to fetch crates|Read-only file system|Permission denied", re.I)


def judge(rc, out, strict):
    """Return (exit code, row to print before the tool's own output)."""
    if rc == 0 or strict or "error[" in out or not UNREACHABLE.search(out):
        return rc, ""
    reason = next(line.strip() for line in out.splitlines() if UNREACHABLE.search(line))
    return 0, f"skip deny (advisory database unreachable: {reason[:160]})"


def selfcheck():
    lock = "[ERROR] failed to acquire advisory database lock: read-only path\n"
    finding = "error[vulnerability]: advisory database says boom\n"
    assert judge(1, lock, False)[0] == 0 and "skip deny" in judge(1, lock, False)[1]
    assert judge(1, lock, True)[0] == 1
    assert judge(1, finding, False)[0] == 1
    assert judge(1, "error: licenses failed\n", False)[0] == 1
    assert judge(0, "", False) == (0, "")
    print("deny selfcheck ok")


if "--selfcheck" in sys.argv:
    selfcheck()
    sys.exit(0)
p = subprocess.run(["cargo", "deny", "check", "-s"], capture_output=True, text=True)
out = p.stdout + p.stderr
rc, row = judge(p.returncode, out, bool(os.environ.get("GITHUB_ACTIONS")))
print(row or out, end="\n" if row else "")
sys.exit(rc)
