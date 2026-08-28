#!/usr/bin/env python3
"""No assistant co-author trailer on a new commit (workflow rule).
Incident: every commit across the five open PRs carried one.
Scans HEAD --not main, so the 50 already on main stay untouched."""
import pathlib, re, subprocess, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

ASSISTANTS = ("claude", "anthropic", "copilot", "cursor", "codex", "gemini", "devin")
NAMED = re.compile(r"\b(" + "|".join(ASSISTANTS) + r")\b", re.I)
# git's own trailer parser, not a regex over the body: the gate's first commit
# named the trailer in its prose, which any line-matching pattern would flag.
FORMAT = "%H%x00%s%x00%(trailers:key=Co-authored-by,valueonly,separator=%x0b)%x1e"


def git(*args):
    return subprocess.run(
        ("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False
    )


def upstream():
    for ref in ("origin/main", "main"):
        if git("rev-parse", "--verify", "--quiet", ref).returncode == 0:
            return ref
    return None


base = upstream()
if base is None:
    print("FAIL trailers")
    print("  no origin/main or main to compare against; a shallow clone cannot")
    print("  run this gate, and passing blind is what it exists to prevent")
    sys.exit(1)

log = git("log", f"--format={FORMAT}", "HEAD", "--not", base)
if log.returncode != 0:
    print("FAIL trailers")
    print(f"  git log failed: {log.stderr.strip()}")
    sys.exit(1)

errs = []
for record in log.stdout.split("\x1e"):
    if not record.strip():
        continue
    sha, subject, authors = record.strip("\n").split("\x00", 2)
    named = [a for a in authors.split("\x0b") if a and NAMED.search(a)]
    if named:
        errs.append(f"{sha[:8]} {subject} — {', '.join(named)}")
if errs:
    errs.append(f"drop the trailer line from each message: git rebase -i {base}")
fail(errs, "trailers")
