#!/usr/bin/env python3
"""cargo-deny, told apart from cargo-deny that could not run.
Incident: inside a read-only or offline sandbox the advisory database lock failed, deny exited 1,
and agents reported the whole guardrails lane unrunnable while every other gate was green. A
database that cannot be reached is a skip row; a finding (`error[...]`) always fails. The CI
environment variable keeps the skip off, matching the surrounding guardrails script. The database path is cargo-deny's own: CARGO_HOME."""
import os
import re
import subprocess
import sys

UNREACHABLE = re.compile(r"failed to (?:acquire advisory database lock|fetch advisory database)", re.I)


def judge(rc, out, strict):
    """Return (exit code, row to print before the tool's own output)."""
    if rc == 0 or strict or "error[" in out or not UNREACHABLE.search(out):
        return rc, ""
    reason = next(line.strip() for line in out.splitlines() if UNREACHABLE.search(line))
    return 0, f"skip deny (advisory database unreachable: {reason})"


def selfcheck():
    lock = "[ERROR] failed to acquire advisory database lock: read-only path\n"
    fetch = "[ERROR] failed to fetch advisory database: network unreachable\n"
    finding = "error[vulnerability]: advisory database says boom\n"
    assert judge(1, lock, False)[0] == 0 and "skip deny" in judge(1, lock, False)[1]
    assert judge(1, fetch, False)[0] == 0 and "skip deny" in judge(1, fetch, False)[1]
    assert judge(1, lock, True)[0] == 1
    assert judge(1, finding, False)[0] == 1
    assert judge(1, "error: licenses failed\n", False)[0] == 1
    for out in (
        "error: `cargo metadata` exited with an error\n",
        "error: failed to read deny.toml: Permission denied\n",
        "error: failed to write report: Read-only file system\n",
        "error: failed to fetch crates\n",
    ):
        assert judge(1, out, False) == (1, "")
    assert judge(0, "", False) == (0, "")
    import pathlib
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        cargo = pathlib.Path(tmp) / "cargo"
        cargo.write_text("#!/bin/sh\nprintf '%s' \"$DENY_OUT\" >&2\nexit 1\n")
        cargo.chmod(0o755)
        env = {**os.environ, "PATH": tmp, "DENY_OUT": lock}
        env.pop("CI", None)
        p = subprocess.run([sys.executable, __file__], capture_output=True, text=True, env=env)
        row = f"skip deny (advisory database unreachable: {lock.strip()})\n"
        assert p.returncode == 0 and p.stdout == row + lock
        env["CI"] = "1"
        p = subprocess.run([sys.executable, __file__], capture_output=True, text=True, env=env)
        assert p.returncode == 1 and p.stdout == lock
    print("deny selfcheck ok")


if "--selfcheck" in sys.argv:
    selfcheck()
    sys.exit(0)
p = subprocess.run(["cargo", "deny", "check", "-s"], capture_output=True, text=True)
out = p.stdout + p.stderr
rc, row = judge(p.returncode, out, bool(os.environ.get("CI")))
if row:
    print(row)
print(out, end="")
sys.exit(rc)
