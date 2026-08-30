#!/usr/bin/env python3
"""Commit subjects and PR titles: one plain imperative sentence, no trailers.
Incident: every commit across the five open PRs carried an assistant co-author
trailer. Scans HEAD --not origin/main, so the landed history stays untouched."""
import argparse, os, pathlib, re, subprocess, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

ASSISTANTS = ("claude", "anthropic", "copilot", "cursor", "codex", "gemini", "devin")
NAMED = re.compile(r"\b(" + "|".join(ASSISTANTS) + r")\b", re.I)
# git's own trailer parser, not a regex over the body: the gate's first commit
# named the trailer in its prose, which any line-matching pattern would flag.
FORMAT = "%H%x00%s%x00%(trailers:only,unfold,separator=%x0b)%x1e"
# The optimizer's run record is landed law (prompt-flywheel plan §8) and git
# appends the sign-off itself; every other key is the drift this gate stops.
TRAILER_OK = re.compile(r"^(Opt-(Run|Delta|Lever|Cases)|Signed-off-by)$", re.I)

LIMIT = 72
# Incident: the subject rules arrived on a lineage 62 commits ahead of main and
# were retroactively red on four good sentences of 73-77 chars. The standard
# starts here; the exclusion self-expires once this lineage lands on main.
STANDARD_STARTS = "6ec141074c880c0fc2f1b4461faca1b1cbe06c35"
PREFIX = "Ratchet: "
GIT_NATIVE = re.compile(r"^(Merge|Revert)\b")
OTHER_PREFIX = re.compile(r"^([A-Za-z][A-Za-z-]*)(\([^)]*\))?!?: ")
NOT_IMPERATIVE = re.compile(
    r"^(Added|Adding|Fixed|Fixes|Fixing|Updated|Updating|Removed|Removing"
    r"|Changed|Changing|Created|Creating|Deleted|Deleting|Renamed|Renaming"
    r"|Moved|Moving|Refactored|Refactoring|Implemented|Implementing"
    r"|Bumped|Bumping|Wrote|Made)\b"
)
SCRATCH = re.compile(r"^(WIP\b|wip\b|fixup!|squash!|amend!)")
MEASURE = re.compile(r"\d+\s*->\s*\d+")
ID = re.compile(r"\b[A-Z]{1,3}\d+\b|§[\d.]+|#\d+")
STOP = frozenset("see per cf ref row rows the of in and for to a an at on with".split())


def substantive(subject):
    words = re.sub(r"[^A-Za-z0-9.'-]+", " ", ID.sub(" ", subject)).split()
    return [w for w in words if w.lower() not in STOP and not w.strip(".-'").replace(".", "").isdigit()]


def subject_errors(subject):
    if GIT_NATIVE.match(subject):
        return []
    errs = []
    if len(subject) > LIMIT:
        errs.append(f"{len(subject)} chars > {LIMIT}")
    if subject.endswith("."):
        errs.append("ends in a period")
    if SCRATCH.match(subject):
        errs.append("WIP/fixup!/squash! subject — squash it before pushing")
    prefix = OTHER_PREFIX.match(subject)
    if prefix and not subject.startswith(PREFIX):
        errs.append(f"unknown prefix {prefix.group(0)!r} — the only one is 'Ratchet: '")
    elif not subject[:1].isupper():
        errs.append("does not start with a capital letter")
    if NOT_IMPERATIVE.match(subject):
        errs.append("past or progressive verb — write the imperative ('Add', not 'Added')")
    if subject.startswith(PREFIX):
        if re.search(r"\d", subject[len(PREFIX) :]) and not MEASURE.search(subject):
            errs.append("a measured ratchet shows its move as 'X -> Y'")
    if ID.search(subject) and len(substantive(subject)) < 2:
        errs.append("ids carry no fact to a cold reader — say what changed, id in the body")
    return errs


def trailer_errors(trailers):
    errs = []
    for line in trailers.split("\x0b"):
        key, _, value = line.partition(":")
        key = key.strip()
        if not key or TRAILER_OK.match(key):
            continue
        if key.lower() == "co-authored-by" and NAMED.search(value):
            errs.append(f"assistant co-author trailer {value.strip()!r}")
        else:
            errs.append(f"unexpected trailer {key!r}")
    return errs


def git(*args):
    return subprocess.run(
        ("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False
    )


def upstream():
    for ref in ("origin/main", "main"):
        if git("rev-parse", "--verify", "--quiet", ref).returncode == 0:
            return ref
    return None


ap = argparse.ArgumentParser()
ap.add_argument("--range", help="git revision range (default: HEAD --not origin/main)")
ap.add_argument("--title", help="check a PR title instead of a commit range")
args = ap.parse_args()

title = args.title or os.environ.get("PR_TITLE")
if title:
    fail([f"{title} — {e}" for e in subject_errors(title.strip())], "pr_title")
    sys.exit(0)

if args.range:
    span, hint = (args.range,), args.range
else:
    hint = "origin/main"
    base = upstream()
    if base is None:
        print("FAIL commit_style")
        print("  no origin/main or main to compare against; a shallow clone cannot")
        print("  run this gate, and passing blind is what it exists to prevent")
        sys.exit(1)
    span = ("HEAD", "--not", base)
    if git("rev-parse", "--verify", "--quiet", STANDARD_STARTS + "^{commit}").returncode == 0:
        span += (STANDARD_STARTS,)

log = git("log", f"--format={FORMAT}", *span)
if log.returncode != 0:
    print("FAIL commit_style")
    print(f"  git log failed: {log.stderr.strip()}")
    sys.exit(1)

errs, seen = [], 0
for record in log.stdout.split("\x1e"):
    if not record.strip():
        continue
    sha, subject, trailers = record.strip("\n").split("\x00", 2)
    seen += 1
    for e in subject_errors(subject) + trailer_errors(trailers):
        errs.append(f"{sha[:8]} {subject} — {e}")
if errs:
    errs.append(f"rewrite the messages: git rebase -i {hint}")
fail(errs, f"commit_style ({seen} commits)")
