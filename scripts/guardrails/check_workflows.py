#!/usr/bin/env python3
"""Two rules a workflow file is refused for, both about the forge reading the file rather than YAML:
a plain value holds no `: ` (the mapping incident below), and a commit message a workflow pushes
carries no skip keyword, which starts no workflow at all for that push.
Incident: `if: ... startsWith(title, 'WIP: ')` failed three review runs before any job existed
("mapping values are not allowed in this context"); the stdlib has no YAML parser."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

KEY = re.compile(r"^\s*(?:-\s+)?[\w.$-]+:(?:\s+(.*))?$")
BLOCK = re.compile(r"^[|>][-+0-9]*\s*(#.*)?$")
NOT_PLAIN = tuple("'\"|>{[&*!#")
SKIP = re.compile(r"\[\s*(?:skip[ _-]?ci|ci[ _-]?skip|no[ _-]?ci|skip[ _-]?actions|actions[ _-]?skip)\s*\]", re.I)


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


def selfcheck(off=""):
    incident = "jobs:\n  review:\n    if: github.head == github.repo && startsWith(github.title, 'WIP: ')\n"
    fixed = "jobs:\n  review:\n    if: >-\n      startsWith(github.title, 'WIP: ')\n"
    fine = "steps:\n  - name: 'quoted: fine'\n  - run: |\n      echo a: b\n      note: a: b\n  - run: echo done # note: ok\n"
    bad = []
    if [n for n, _ in problems(incident, off)] != [3]:
        bad.append("the incident's plain `if:` holding 'WIP: ' is not refused")
    if problems(fixed, off) or problems(fine, off):
        bad.append("a folded value, a quoted one or a `run: |` block's text or a trailing comment is refused")
    quiet = 'record:\n  - run: |\n      git add docs\n      git commit -q -m "record" -m "[skip ci]"\n'
    if [n for n, _ in silenced(quiet, off)] != [4]:
        bad.append("a commit message carrying [skip ci] is not refused")
    if silenced('# a comment naming [skip ci] is not a commit\nrun: echo one\n', off):
        bad.append("a comment naming the keyword is refused")
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("plain", "blocks", "comments", "silenced"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        fail(errs, "workflows selfcheck")
        sys.exit(0)
    files = sorted((ROOT / ".forgejo/workflows").glob("*.y*ml"))
    errs = [f"{f.relative_to(ROOT)}:{n}: `{v}` is a plain YAML value holding `: `, which starts a mapping; "
            f"quote it or fold it with `>-`" for f in files for n, v in problems(f.read_text())]
    errs += [f"{f.relative_to(ROOT)}:{n}: `{v}` starts no workflow for the push that carries it, the mirror's "
             f"run included; skip the lane that must not re-run instead, by author or by path"
             for f in files for n, v in silenced(f.read_text())]
    fail(errs, "workflows")
