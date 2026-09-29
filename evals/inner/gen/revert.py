"""Real past fixes, redone (SWE-smith's PR mirroring): a commit from a repo's own history that
changed the library and its tests, with the library change undone. The commit's message is the
request and the commit's tests grade the redo: each test it turned green is a point, plus one when
nothing else regressed. The repos and their history stay outside this repository (INNER_REPOS,
full clones); the workspace has no .git to read the fix back out of.

Levels: 1 keeps the commit's tests in the workspace and names the failing ones; 2 hides them and
shows their failure lines; 3 is a bigger change on level 2's terms. Showing nothing but the message
was not a level: a hidden test that pins an exact error string cannot be met from prose (probe,
2026-09-28: tomli's "Expected str object, not 'bytes'")."""
import json
import random
import shlex
import shutil
import subprocess
import tempfile
from pathlib import Path

from . import mutate
from .mutate import _dead, _hide, _module_path, _runner, _suite, grade

CACHE = mutate.CACHE.parent / "revert"
DEPTH = 600  # commits back from the pinned one
SIZES = {1: (2, 80), 2: (2, 80), 3: (40, 300)}  # library lines the commit changed, by level
FIXED = (1, 20)  # tests the commit turned green
# mutate's repos less tomli, whose slot pyparsing takes: a repo whose later release the agent can
# import answers its own history, and yi's kernel venv ships tomli 2.4.1 (probe, 2026-09-28: a
# level-3 rollout called it to read the error it had to write). None of these is in that venv.
REPOS = {name: spec for name, spec in mutate.REPOS.items() if name != "tomli"} | {
    "pyparsing": {"path": mutate.HOME / "pyparsing", "commit": "f803ae8", "src": "pyparsing", "tests": "tests"},
}


def available():
    return all(Path(spec["path"]).is_dir() and _git(spec["path"], "rev-parse", "--is-shallow-repository").strip()
               == "false" for spec in REPOS.values())


def _git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True, check=True).stdout


def _candidates(name):
    """[commit, library lines changed] for each non-merge commit that changed Python under the
    library and anything under the tests, newest first."""
    spec = REPOS[name]
    head = spec.get("commit") or "HEAD"
    cached = CACHE / f"{name}-{head}-changes.json"
    if cached.is_file():
        return json.loads(cached.read_text())
    found = []
    # One parent: no merges, and no root commit to diff against nothing.
    for commit in _git(spec["path"], "rev-list", "--min-parents=1", "--max-parents=1", f"-n{DEPTH}", head).split():
        files = _git(spec["path"], "diff-tree", "--no-commit-id", "--name-only", "-r", commit).split()
        if not any(f.startswith(spec["src"] + "/") and f.endswith(".py") for f in files):
            continue
        if not any(f.startswith(spec["tests"] + "/") for f in files):
            continue
        stat = _git(spec["path"], "diff", "--numstat", f"{commit}^", commit, "--", spec["src"]).splitlines()
        lines = sum(int(a) + int(b) for a, b, _ in (row.split("\t", 2) for row in stat) if a != "-")
        if lines:
            found.append([commit, lines])
    CACHE.mkdir(parents=True, exist_ok=True)
    cached.write_text(json.dumps(found))
    return found


def _tree(name, commit):
    """The repo's tree at `commit`, extracted once."""
    root = CACHE / "trees" / f"{name}-{commit[:12]}"
    if not root.is_dir():
        partial = root.with_suffix(".partial")
        shutil.rmtree(partial, ignore_errors=True)
        partial.mkdir(parents=True)
        subprocess.run(f"git -C {shlex.quote(str(REPOS[name]['path']))} archive {commit} | tar -x -C "
                       f"{shlex.quote(str(partial))}", shell=True, check=True)
        partial.rename(root)
    return root


def _undo(name, commit):
    """{library file: its text before the commit}, and the library files the commit added."""
    spec = REPOS[name]
    before, added = {}, []
    for row in _git(spec["path"], "diff", "--name-status", "--no-renames", f"{commit}^", commit, "--",
                    spec["src"]).splitlines():
        status, path = row.split("\t", 1)
        if status == "A":
            added.append(path)
        else:
            before[path] = _git(spec["path"], "show", f"{commit}^:{path}")
    return before, added


def _judge(name, commit):
    """Whether the commit makes a task, computed once per commit and cached: the tests that pass
    after it, the ones that fail with its library change undone, and their failure lines."""
    cached = CACHE / f"{name}-{commit[:12]}.json"
    if cached.is_file():
        return json.loads(cached.read_text())
    spec = REPOS[name]
    before, added = _undo(name, commit)
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp) / "repo"
        shutil.copytree(_tree(name, commit), work)
        after = _suite(work, spec)
        passing = {t for t, outcome in after.items() if outcome == "ok"}
        for path, text in before.items():
            (work / path).write_text(text)
        for path in added:
            (work / path).unlink()
        results, notes = _suite(work, spec, notes=True)
        f2p = sorted(t for t in passing if results.get(t) != "ok")
    verdict = {"repo": name, "commit": commit, "ok": FIXED[0] <= len(f2p) <= FIXED[1], "f2p": f2p,
               "p2p": sorted(passing - set(f2p)), "drop": _dead(after),
               "notes": {t: notes.get(t, "") for t in f2p}}
    cached.write_text(json.dumps(verdict))
    return verdict


def _plan(seed, level):
    """Seed n takes repo n mod 3 and that repo's (n div 3)-th usable commit of the level's size, in a
    fixed shuffle of its candidates; levels 2-3 skip a commit whose tests cannot be cut out."""
    name = sorted(REPOS)[seed % len(REPOS)]
    low, high = SIZES[level]
    candidates = [commit for commit, lines in _candidates(name) if low <= lines <= high]
    random.Random(f"revert:{name}").shuffle(candidates)
    want = seed // len(REPOS)
    for commit in candidates:
        verdict = _judge(name, commit)
        if verdict["ok"] and (level == 1 or _hide(_tree(name, commit), REPOS[name], verdict["f2p"]) is not None):
            if want == 0:
                return verdict
            want -= 1
    raise RuntimeError(f"{name} has too few usable commits for seed {seed} at level {level}")


def make(seed, level=1):
    plan = _plan(seed, level)
    name, commit, spec = plan["repo"], plan["commit"], REPOS[plan["repo"]]
    tree = _tree(name, commit)
    before, added = _undo(name, commit)
    patch = dict(before)
    message = _git(spec["path"], "log", "-1", "--format=%B", commit).strip()
    shown = plan["f2p"][:5]
    ask = (f"This directory is the {name} library, just before a change its maintainers made to the library "
           f"code under {spec['src']}/. Their description of the change:\n\n{message}\n\n")
    if level == 1:
        tail = (f"Their tests for it are in the suite and fail now, including: {', '.join(shown)}. Make the change "
                "so they pass without breaking the rest. Do not modify the tests. Run the suite with ./run_tests.sh.")
    else:
        patch.update(_hide(tree, spec, plan["f2p"]))
        reports = "\n".join(f"- {t}: {plan['notes'][t]}" for t in shown)
        tail = ("Their tests for it are not in this directory, so ./run_tests.sh cannot show what is missing. "
                + f"Those tests failed with:\n{reports}\n"
                + "Make the change in the library code, and check it yourself. Do not modify the tests.")
    spec_at = {**spec, "path": tree}
    drop = added + [_module_path(m, spec_at) for m in plan["drop"] if _module_path(m, spec_at)]
    return {"prompt": ask + tail, "tree": str(tree), "patch": patch, "files": {"run_tests.sh": _runner(spec)},
            "drop": drop, "timeoutSec": 900}


def check(seed, workspace, level=1):
    """The commit's own tests over the workspace's library."""
    plan = _plan(seed, level)
    return grade(_tree(plan["repo"], plan["commit"]), workspace, REPOS[plan["repo"]], plan["f2p"], plan["p2p"])


def solve(seed, workspace, level=1):
    plan = _plan(seed, level)
    src = REPOS[plan["repo"]]["src"]
    shutil.rmtree(Path(workspace) / src)
    shutil.copytree(_tree(plan["repo"], plan["commit"]) / src, Path(workspace) / src)
