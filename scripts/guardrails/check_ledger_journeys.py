#!/usr/bin/env python3
"""Every feature-ledger row names a journey test, and the names live in docs/ledger-journeys.txt.
Incident: the ledger row is one 2-4 KB line, so two PRs adding different journey tests to one
feature conflicted on it and were merged by hand. One `feature | test` line each, union-merged
(.gitattributes), lets both append without a conflict."""
import re, subprocess, sys, tempfile, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

FILE, ARCH = "docs/ledger-journeys.txt", "docs/ARCHITECTURE.md"
CELL = re.compile(r"(?<!\\)\|")


def ledger_rows(arch):
    keys, inside, started = [], False, False
    for line in arch.splitlines():
        if line.startswith("#"):
            inside, started = line.lstrip("#").strip() == "Feature ledger", False
        elif inside and line.startswith("|"):
            if not set(line) - set("|-: "):
                started = True
            elif started:
                keys.append((CELL.split(line)[1].strip(), line))
    return keys


def ledger_keys(arch):
    return [k for k, _ in ledger_rows(arch)]


def problems(arch, journeys, off=""):
    keys, out, named = ledger_keys(arch), [], set()
    if off != "inline":
        out += [f"feature-ledger row `{k}` keeps a journey: clause in its note; move it to {FILE}"
                for k, row in ledger_rows(arch) if re.search(r"journeys?:", row)]
    for n, line in enumerate(journeys.splitlines(), 1):
        key, sep, test = line.partition(" | ")
        if not sep or "`" not in test:
            out.append(f"{FILE}:{n}: not `feature | `test``: {line[:60]}")
        elif off != "stale" and key not in keys:
            out.append(f"{FILE}:{n}: no feature-ledger row named `{key}`")
        named.add(key)
    if off != "bare":
        out += [f"feature-ledger row `{k}` names no journey test in {FILE}" for k in keys if k not in named]
    return out


def merges_clean(attributes):
    """Two branches each append a different line to one file; True when git merges them unaided."""
    with tempfile.TemporaryDirectory() as tmp:
        def git(*a):
            return subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false",
                                   "-C", tmp, *a], capture_output=True, text=True)
        git("init", "-q", "-b", "main")
        (pathlib.Path(tmp) / ".gitattributes").write_text(attributes)
        (pathlib.Path(tmp) / "docs").mkdir()
        (pathlib.Path(tmp) / FILE).write_text("loop | `a::one`\n")
        git("add", "."), git("commit", "-qm", "base")
        for name, line in (("x", "loop | `a::two`\n"), ("y", "loop | `a::three`\n")):
            git("checkout", "-qb", name, "main")
            with open(pathlib.Path(tmp) / FILE, "a") as f:
                f.write(line)
            git("commit", "-qam", name)
        git("checkout", "-q", "x")
        return git("merge", "-q", "--no-edit", "y").returncode == 0


def selfcheck(off=""):
    arch = "## Feature ledger\n\n| feature | § |\n|---|---|\n| loop | 4 |\n| other | 5 |\n"
    ok = "loop | `a::one` (T2)\nother | `b::two`\n"
    bad = []
    if problems(arch, ok, off):
        bad.append("a ledger that names a test for every row is refused")
    if len(problems(arch, "loop | `a::one`\n", off)) != 1:
        bad.append("a row with no journey line is not refused")
    if len(problems(arch, ok + "ghost | `a::one`\n", off)) != 1:
        bad.append("a journey line for a row that is not in the ledger is not refused")
    if len(problems(arch, ok + "loop: a::one\n", off)) != 1:
        bad.append("a malformed line is not refused")
    if len(problems(arch.replace("| 5 |", "| 5 | journey: `x::y` |"), ok, off)) != 1:
        bad.append("a journey: clause left in a ledger note is not refused")
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("bare", "stale", "inline"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        attrs = (ROOT / ".gitattributes").read_text()
        if not merges_clean(attrs):
            errs.append("two branches adding different journey lines to docs/ledger-journeys.txt conflict")
        if merges_clean(attrs.replace(f"{FILE} merge=union", "")):
            errs.append("the merge fixture passes without the union rule, so it refutes nothing")
        fail(errs, "ledger_journeys selfcheck")
        sys.exit(0)
    fail(problems((ROOT / ARCH).read_text(), (ROOT / FILE).read_text()), "ledger_journeys")
