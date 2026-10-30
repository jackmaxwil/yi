#!/usr/bin/env python3
"""Three rules a workflow file is refused for, all about the forge reading the file rather than YAML:
a plain value holds no `: ` (the mapping incident below), a commit message a workflow pushes carries
no skip keyword, which starts no workflow at all for that push, and a review job never runs on a
gate job's runner lane.
Incident: `if: ... startsWith(title, 'WIP: ')` failed three review runs before any job existed
("mapping values are not allowed in this context"); the stdlib has no YAML parser."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

KEY = re.compile(r"^\s*(?:-\s+)?[\w.$-]+:(?:\s+(.*))?$")
BLOCK = re.compile(r"^[|>][-+0-9]*\s*(#.*)?$")
NOT_PLAIN = tuple("'\"|>{[&*!#")
SKIP = re.compile(r"\[\s*(?:skip[ _-]?ci|ci[ _-]?skip|no[ _-]?ci|skip[ _-]?actions|actions[ _-]?skip)\s*\]", re.I)

JOBS = re.compile(r"^  ([\w-]+):\s*$")
RUNNER = re.compile(r"^    runs-on:\s*(\S+)\s*$")


def problems(text, off=""):
    out, block = [], None
    for n, line in enumerate(text.splitlines(), 1):
        indent = len(line) - len(line.lstrip())
        if block is not None:
            if not line.strip() or indent > block:
                continue
            block = None
        m = KEY.match(line)
        value = (m.group(1) or "") if m else ""
        value = (value if off == "comments" else value.split(" #")[0]).rstrip()
        if not value:
            continue
        if BLOCK.match(value):
            block = None if off == "blocks" else indent
        elif off != "plain" and not value.startswith(NOT_PLAIN) and ": " in value:
            out.append((n, value))
    return out


def silenced(text, off=""):
    """A skip keyword in a commit message: the forge starts no workflow for that push, so main moves
    with no run to see it (#1771 was the recorder's push, which the mirror needed to run for)."""
    if off == "silenced":
        return []
    return [(n, line.strip()) for n, line in enumerate(text.splitlines(), 1)
            if not line.lstrip().startswith("#") and SKIP.search(line)]


def lanes(text):
    """{job: runner label} from a workflow's `jobs:` block; a job whose `runs-on` is an expression has none."""
    out, job = {}, None
    for line in text.splitlines():
        if line.startswith("jobs:"):
            job = None
        elif JOBS.match(line):
            job = JOBS.match(line).group(1)
        elif job and RUNNER.match(line):
            out[job] = RUNNER.match(line).group(1)
    return out


def shared_lane(gate, review, off=""):
    """(job, lane) of every review job on a lane a gate job of the same estate runs on."""
    if off == "lane":
        return []
    gate_lanes = set(lanes(gate).values())
    return sorted((job, lane) for job, lane in lanes(review).items() if lane in gate_lanes)


def selfcheck(off=""):
    incident = "jobs:\n  review:\n    if: github.head == github.repo && startsWith(github.title, 'WIP: ')\n"
    fixed = "jobs:\n  review:\n    if: >-\n      startsWith(github.title, 'WIP: ')\n"
    fine = "steps:\n  - name: 'quoted: fine'\n  - run: |\n      echo a: b\n      note: a: b\n  - run: echo done # note: ok\n"
    quiet = 'record:\n  - run: |\n      git add docs\n      git commit -q -m "record" -m "[skip ci]"\n'
    gate = "jobs:\n  gate:\n    runs-on: ubuntu-latest\n"
    shared = "jobs:\n  review:\n    runs-on: ubuntu-latest\n"
    apart = "jobs:\n  review:\n    runs-on: llm\n"
    bad = []
    if [n for n, _ in problems(incident, off)] != [3]:
        bad.append("the incident's plain `if:` holding 'WIP: ' is not refused")
    if problems(fixed, off) or problems(fine, off):
        bad.append("a folded value, a quoted one or a `run: |` block's text or a trailing comment is refused")
    if [n for n, _ in silenced(quiet, off)] != [4]:
        bad.append("a commit message carrying [skip ci] is not refused")
    if silenced('# a comment naming [skip ci] is not a commit\nrun: echo one\n', off):
        bad.append("a comment naming the keyword is refused")
    if shared_lane(gate, shared, off) != [("review", "ubuntu-latest")]:
        bad.append("a review job on the gate's own runner lane is not refused")
    if shared_lane(gate, apart, off):
        bad.append("a review job on a lane of its own is refused")
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("plain", "blocks", "comments", "silenced", "lane"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        fail(errs, "workflows selfcheck")
        sys.exit(0)
    files = sorted((ROOT / ".forgejo/workflows").glob("*.y*ml"))
    gate = (ROOT / ".forgejo/workflows/pr.yml").read_text()
    review = (ROOT / ".forgejo/workflows/review.yml").read_text()
    errs = [f"{f.relative_to(ROOT)}:{n}: `{v}` is a plain YAML value holding `: `, which starts a mapping; "
            f"quote it or fold it with `>-`" for f in files for n, v in problems(f.read_text())]
    errs += [f"{f.relative_to(ROOT)}:{n}: `{v}` starts no workflow for the push that carries it, the mirror's "
             f"run included; skip the lane that must not re-run instead, by author or by path"
             for f in files for n, v in silenced(f.read_text())]
    errs += [f".forgejo/workflows/review.yml: job {job} runs on {lane}, the lane a pr.yml gate job runs on; "
             "a round waits an hour or two on the model at a few percent of one CPU, so the gates queued behind "
             "it for up to four hours (#1310): give the round a lane of its own"
             for job, lane in shared_lane(gate, review)]
    fail(errs, "workflows")
