"""Bug injection into real repos (SWE-smith's method): a seeded, span-precise mutation of the
library code, kept only when the repo's own suite turns red, graded by the pristine tests. The
repos are pure-Python libraries with stdlib-unittest suites, cloned outside this repository at a
pinned commit (INNER_REPOS, default ~/Development/yi-ref/inner-repos); nothing of theirs is
committed here. The workspace has no .git, so the bug cannot be read back out of a diff."""
import ast
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HOME = Path(os.environ.get("INNER_REPOS", Path.home() / "Development" / "yi-ref" / "inner-repos"))
REPOS = {
    "markdown": {"path": HOME / "markdown", "commit": "0ffbf00", "src": "markdown", "tests": "tests"},
    "more-itertools": {"path": HOME / "more-itertools", "commit": "790bb0b", "src": "more_itertools", "tests": "tests"},
    "tomli": {"path": HOME / "tomli", "commit": "5a77b12", "src": "src/tomli", "tests": "tests", "pythonpath": "src"},
}
CACHE = Path.home() / ".cache" / "yi-inner" / "mutate"
def _python():
    """INNER_PYTHON, else the newest uv-managed CPython, else this interpreter. Incident: plain
    `python3` here is Xcode's 3.9, under which Markdown's pristine suite had 369 errors."""
    if os.environ.get("INNER_PYTHON"):
        return os.environ["INNER_PYTHON"]
    found = sorted(Path.home().glob(".local/share/uv/python/cpython-3.1[3-9]*/bin/python3.1[3-9]"))
    return str(found[-1]) if found else sys.executable


PYTHON = _python()
IGNORE = shutil.ignore_patterns(".git", ".venv", "__pycache__", "*.pyc")
BROKEN = (1, 12)  # a mutation must turn between 1 and 12 passing tests red
SWAPS = {"<": "<=", "<=": "<", ">": ">=", ">=": ">", "==": "!=", "!=": "==", "+": "-", "-": "+",
         "and": "or", "or": "and"}
# One unittest run, reported as {test id: ok | fail | skip}; an import error counts as a failed id.
# Each failure's first message line is kept too: level 3 shows it in place of the test.
SUITE = """import json, re, sys, unittest
results, notes = {}, {}
def note(t, e):
    first = (str(e[1]).splitlines() or [""])[0]
    notes[t.id()] = re.sub(r"0x[0-9a-f]+", "0x...", f"{e[0].__name__}: {first}")[:200]
class R(unittest.TestResult):
    def addSuccess(self, t): results[t.id()] = "ok"
    def addFailure(self, t, e): results[t.id()] = "fail"; note(t, e)
    def addError(self, t, e): results[t.id()] = "fail"; note(t, e)
    def addSkip(self, t, r): results[t.id()] = "skip"
    def addExpectedFailure(self, t, e): results[t.id()] = "ok"
    def addUnexpectedSuccess(self, t): results[t.id()] = "fail"
unittest.TestLoader().discover(start_dir=sys.argv[1], top_level_dir=".").run(R())
print(json.dumps({"results": results, "notes": notes}))
"""


def available():
    return all(Path(spec["path"]).is_dir() for spec in REPOS.values())


def _repo(seed):
    name = sorted(REPOS)[seed % len(REPOS)]
    return name, REPOS[name]


def _suite(root, spec, timeout=180, notes=False):
    # Incident: a same-size edit ("0" to "1") inside the source file's mtime second reused the stale
    # .pyc, so a mutation, or its revert, silently did not run; no bytecode is written or read.
    env = {**os.environ, "PYTHONPATH": str(Path(root) / spec.get("pythonpath", ".")), "PYTHONDONTWRITEBYTECODE": "1"}
    for stale in Path(root).rglob("__pycache__"):
        shutil.rmtree(stale, ignore_errors=True)
    try:
        done = subprocess.run([PYTHON, "-B", "-c", SUITE, spec["tests"]], cwd=root, env=env, capture_output=True,
                              text=True, timeout=timeout)
        out = json.loads(done.stdout.strip().splitlines()[-1])
        return (out["results"], out["notes"]) if notes else out["results"]
    except (subprocess.TimeoutExpired, ValueError, IndexError, KeyError):
        return ({}, {}) if notes else {}


def _sites(repo, spec):
    """Every single-line operator or small integer in the library code a mutation can flip, as
    (file, line, byte start, byte end, old, new), in a fixed order."""
    sites = []
    src = Path(repo) / spec["src"]
    for path in sorted(src.rglob("*.py")):
        text = path.read_text()
        try:
            tree = ast.parse(text)
        except SyntaxError:
            continue
        lines = text.splitlines(keepends=True)
        relative = str(path.relative_to(repo))

        def between(a, b):
            if a.end_lineno != b.lineno:
                return
            raw = lines[b.lineno - 1].encode()
            segment = raw[a.end_col_offset:b.col_offset].decode(errors="replace")
            token = segment.strip()
            if token in SWAPS:
                start = a.end_col_offset + len(segment.encode()) - len(segment.lstrip().encode())
                sites.append((relative, b.lineno, start, start + len(token.encode()), token, SWAPS[token]))

        for node in ast.walk(tree):
            if isinstance(node, ast.Compare) and len(node.ops) == 1:
                between(node.left, node.comparators[0])
            elif isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Add, ast.Sub)):
                between(node.left, node.right)
            elif isinstance(node, ast.BoolOp) and len(node.values) == 2:
                between(node.values[0], node.values[1])
            elif isinstance(node, ast.Constant) and type(node.value) is int and 0 <= node.value <= 64 \
                    and node.lineno == node.end_lineno:
                raw = lines[node.lineno - 1].encode()
                old = raw[node.col_offset:node.end_col_offset].decode(errors="replace")
                if old == str(node.value):
                    sites.append((relative, node.lineno, node.col_offset, node.end_col_offset, old, str(node.value + 1)))
    return sites


def _apply(text, site):
    _file, line, start, end, _old, new = site
    lines = text.splitlines(keepends=True)
    raw = lines[line - 1].encode()
    lines[line - 1] = (raw[:start] + new.encode() + raw[end:]).decode()
    return "".join(lines)


def _hide(root, spec, tests):
    """{test file: its text without the given tests}, or None when one is not a literal method of
    its class (a generated or inherited test cannot be cut out without taking others with it)."""
    wanted = {}
    for test in tests:
        module, cls, method = test.rsplit(".", 2)
        wanted.setdefault(_module_path(module, {"path": root}), []).append((cls, method))
    if None in wanted:
        return None
    files = {}
    for relative, methods in wanted.items():
        text = (Path(root) / relative).read_text()
        tree = ast.parse(text)
        cut = set()
        for cls, method in methods:
            found = [f for c in tree.body if isinstance(c, ast.ClassDef) and c.name == cls for f in c.body
                     if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef)) and f.name == method]
            if not found:
                return None
            first = min([found[0].lineno] + [d.lineno for d in found[0].decorator_list])
            cut.update(range(first - 1, found[0].end_lineno))
        body = "".join(line for i, line in enumerate(text.splitlines(keepends=True)) if i not in cut)
        try:
            ast.parse(body)
        except SyntaxError:  # a class left with no body
            return None
        files[relative] = body
    return files


def _plan(seed, level):
    """The chosen sites and the tests they break, computed once per (repo, seed, level) and cached:
    finding them runs the suite, and a task must be the same task every time. Level n plants n bugs;
    level 3 plants two and hides the tests they break, so the task is a report of failures against a
    suite that passes, as an issue is."""
    name, spec = _repo(seed)
    key = hashlib.sha256(json.dumps([name, spec.get("commit"), str(spec["path"]), seed, level]).encode()).hexdigest()[:16]
    cached = CACHE / f"{name}-{seed}-{level}-{key}.json"
    if cached.is_file():
        return json.loads(cached.read_text())
    repo = Path(spec["path"])
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "repo"
        shutil.copytree(repo, work, ignore=IGNORE)
        started = time.monotonic()
        pristine = _suite(work, spec)
        # Incident: a constant bumped inside a loop bound never returned, and each such candidate
        # waited out the full timeout; a candidate gets four times the pristine run, at least 10 s.
        budget = max(10.0, 4 * (time.monotonic() - started))
        passing = {t for t, outcome in pristine.items() if outcome == "ok"}
        sites = _sites(work, spec)
        bugs = 2 if level == 3 else level
        random.Random(f"mutate:{name}:{seed}:{level}").shuffle(sites)
        chosen = []
        for site in sites[:300]:
            # Distinct bugs: another file, or the same file more than 30 lines away (more-itertools
            # keeps its code in two files, so "another file" alone refused its level 2).
            if any(c[0] == site[0] and abs(c[1] - site[1]) <= 30 for c in chosen):
                continue
            path = work / site[0]
            before = path.read_text()
            path.write_text(_apply(before, site))
            results = _suite(work, spec, budget)
            broke = ({t for t, outcome in results.items() if outcome != "ok"} & passing) if results else set()
            path.write_text(before)
            if BROKEN[0] <= len(broke) <= BROKEN[1] and (level < 3 or _hide(work, spec, broke) is not None):
                chosen.append(site)
                if len(chosen) == bugs:
                    break
        if len(chosen) < bugs:
            raise RuntimeError(f"no {bugs} mutations of {name} break 1-12 tests within 300 sites (seed {seed})")
        for site in chosen:
            path = work / site[0]
            path.write_text(_apply(path.read_text(), site))
        results, notes = _suite(work, spec, notes=True)
        broken = sorted({t for t, outcome in results.items() if outcome != "ok"} & passing)
        if level == 3:
            hidden = _hide(work, spec, broken)
            for relative, body in (hidden or {}).items():
                (work / relative).write_text(body)
            visible = {t for t, outcome in _suite(work, spec).items() if outcome != "ok"} & passing
            if hidden is None or visible:
                raise RuntimeError(f"the tests {name} seed {seed} breaks cannot all be hidden: {sorted(visible)[:3]}")
    # A test module none of whose tests pass on the pristine repo (an import it cannot satisfy, as
    # Markdown's test_apis needs PyYAML) stays out of the workspace: the agent is asked to make the
    # whole suite pass, and those were never its to fix.
    modules = {}
    for test, outcome in pristine.items():
        # An import failure is reported as `unittest.loader._FailedTest.<module>`.
        failed = test.startswith("unittest.loader._FailedTest.")
        module = test.removeprefix("unittest.loader._FailedTest.") if failed else test.rsplit(".", 2)[0]
        modules.setdefault(module, set()).add(outcome)
    dead = sorted(m for m, outcomes in modules.items() if "ok" not in outcomes and "skip" not in outcomes)
    plan = {"repo": name, "sites": chosen, "f2p": broken, "p2p": sorted(passing - set(broken)), "drop": dead,
            "notes": {t: notes.get(t, "") for t in broken}}
    CACHE.mkdir(parents=True, exist_ok=True)
    cached.write_text(json.dumps(plan))
    return plan


def make(seed, level=1):
    plan = _plan(seed, level)
    spec = REPOS[plan["repo"]]
    repo = Path(spec["path"])
    patch = {}
    for site in plan["sites"]:
        patch[site[0]] = _apply(patch.get(site[0]) or (repo / site[0]).read_text(), site)
    shown = plan["f2p"][:5]
    if level < 3:
        prompt = (f"This directory is the {plan['repo']} library. Its test suite fails: a bug was introduced in the "
                  f"library code under {spec['src']}/. Failing tests include: {', '.join(shown)}. Find and fix the "
                  "bug(s) in the library code so the whole suite passes. Do not modify the tests. Run the suite "
                  "with ./run_tests.sh.")
    else:
        patch.update(_hide(repo, spec, plan["f2p"]))
        reports = "\n".join(f"- {t}: {plan['notes'][t]}" for t in shown)
        prompt = (f"This directory is the {plan['repo']} library. Bugs were introduced in the library code under "
                  f"{spec['src']}/. Its maintainers' tests caught them with these failures:\n{reports}\nThose tests "
                  "are not in this directory, so ./run_tests.sh passes as it is. Reproduce the failures, then fix "
                  "the library code. Do not modify the tests.")
    runner = (f"#!/bin/sh\ncd \"$(dirname \"$0\")\" && PYTHONPATH={spec.get('pythonpath', '.')} exec {PYTHON} "
              f"-m unittest discover -s {spec['tests']} -t . \"$@\"\n")
    return {"prompt": prompt, "tree": str(repo), "patch": patch, "files": {"run_tests.sh": runner},
            "drop": [_module_path(m, spec) for m in plan.get("drop", []) if _module_path(m, spec)],
            "timeoutSec": 600}


def _module_path(module, spec):
    """`tests.test_apis` -> `tests/test_apis.py`, when that file exists in the repo."""
    path = module.replace(".", "/") + ".py"
    return path if (Path(spec["path"]) / path).is_file() else None


def check(seed, workspace, level=1):
    """Broken tests fixed, plus one point when nothing that passed before fails now; the tests are
    the pristine repo's, whatever the workspace did to its own."""
    plan = _plan(seed, level)
    spec = REPOS[plan["repo"]]
    total = len(plan["f2p"]) + 1
    library = Path(workspace) / spec["src"]
    if not library.is_dir():
        return 0, total
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "repo"
        shutil.copytree(spec["path"], work, ignore=IGNORE)
        shutil.rmtree(work / spec["src"])
        shutil.copytree(library, work / spec["src"], ignore=IGNORE)
        results = _suite(work, spec)
    fixed = sum(1 for t in plan["f2p"] if results.get(t) == "ok")
    clean = all(results.get(t) == "ok" for t in plan["p2p"])
    return fixed + int(clean and bool(results)), total


def solve(seed, workspace, level=1):
    plan = _plan(seed, level)
    repo = Path(REPOS[plan["repo"]]["path"])
    for relative in {site[0] for site in plan["sites"]}:
        (Path(workspace) / relative).write_text((repo / relative).read_text())
