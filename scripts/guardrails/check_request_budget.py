#!/usr/bin/env python3
"""Request-prefix budget and tool-surface lock, ratcheted (design §21, D188): the
system block plus the tool table are what every turn pays before the
conversation starts, and the surface a model reads is locked so a PR that
changes it owes the sections check_pr_metadata.py names. Measured by
`yi-runtime`'s request_budget test, which owns the fixture: the table is the
one a session registers, so the lock pins what a model call can name. Each
role's first request in a bare workspace (root, leaf root child, reader,
worker) is ratcheted too: the root's prompt alone let a child prefix grow
unseen."""
import hashlib
import json
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from _common import ROOT, BASE, fail  # noqa: E402
from pr_body import LOCK_MISSING, surface_diff  # noqa: E402

LOCK = BASE / "tool_surface.json"
ROLES = BASE / "role_prefix.json"
BARE_ROLES = ("root", "leaf", "leaf_orchestrate", "reader", "worker")
ROLE_LINE = re.compile(
    r"^ROLE_PREFIX workspace=(\w+) role=(\w+) system=\d+ tools=\d+ total=(\d+)", re.M)


def measure():
    """One cargo run prices the prefix and renders the surface, so the lock
    rides a test the guardrails lane already pays for."""
    # --workspace for the reason check_behavior.py gives: one build shared with the test lane.
    out = subprocess.run(
        ["cargo", "test", "-q", "--workspace", "--test", "request_budget",
         "report_the_prefix_size", "--", "--nocapture"],
        cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0:
        fail([f"request_budget test failed:\n{out.stdout}{out.stderr}"], "request_budget")
    found = re.search(r"REQUEST_PREFIX system=(\d+) tools=(\d+) total=(\d+)", out.stdout)
    if found is None:
        fail(["request_budget test printed no REQUEST_PREFIX line"], "request_budget")
    surface = {}
    for line in out.stdout.splitlines():
        if line.startswith("TOOL_SURFACE "):
            record = json.loads(line[len("TOOL_SURFACE "):])
            surface[record["key"]] = hashlib.sha256(record["text"].encode()).hexdigest()
    if not surface:
        fail(["request_budget test printed no TOOL_SURFACE lines"], "request_budget")
    return (tuple(int(found.group(n)) for n in (1, 2, 3)), surface, role_rows(out.stdout))


def role_rows(stdout):
    return {(ws, role): int(total) for ws, role, total in ROLE_LINE.findall(stdout)}


def role_problems(budget, rows):
    """Only the bare rows gate: the lane and main rows read this repo's own instruction
    files at test time, which CI checks out differently (no CLAUDE.md)."""
    errs = []
    for role in BARE_ROLES:
        got, limit = rows.get(("bare", role)), budget.get(role)
        if got is None:
            errs.append(f"request_budget test printed no ROLE_PREFIX line for the bare {role}")
        elif limit is None:
            errs.append(f"role_prefix.json has no {role}: rerun --update in its own Ratchet commit")
        elif got > limit:
            errs.append(f"bare {role} prefix {got} > budget {limit}")
    return errs


def surface_delta(lock, surface):
    """(added, changed, removed) between the locked and the measured surface.
    added/changed are pr_body's surface_diff, the one implementation; the lock
    also reports removals, which a PR body owes nothing for."""
    added, changed = surface_diff(lock, surface)
    removed = sorted(key for key in lock if key not in surface)
    return added, changed, removed


def surface_problems(lock, surface):
    if lock is None:
        return [LOCK_MISSING]
    added, changed, removed = surface_delta(lock, surface)
    if not (added or changed or removed):
        return []
    show = lambda keys: ", ".join(keys) if keys else "—"
    return [
        f"tool surface changed: changed {show(changed)}; added {show(added)}; removed {show(removed)}",
        "  rerun check_request_budget.py --update in its own Ratchet commit;",
        "  the PR body then owes the sections check_pr_metadata.py names",
    ]


def selfcheck():
    """The lock judged by its own cases: a drift it stays silent on is a gate
    that only looks like one."""
    locked = {"tool:read": "a", "tool:bash": "b", "extra:openpyxl": "c"}
    missing = surface_problems(None, locked)
    assert missing and "--update" in missing[0], "a deleted lock must fail, naming how to write it"
    assert surface_problems(locked, dict(locked)) == []
    problems = surface_problems(locked, {**locked, "tool:read": "x"})
    assert "changed tool:read" in problems[0] and "added —" in problems[0], problems
    problems = surface_problems(locked, {**locked, "extra:foo": "d"})
    assert "added extra:foo" in problems[0], problems
    problems = surface_problems(locked, {"tool:read": "a", "tool:bash": "b"})
    assert "removed extra:openpyxl" in problems[0], problems
    problems = surface_problems({"tool:read": "a"}, {"tool:write": "a"})
    assert "changed —" in problems[0] and "added tool:write" in problems[0], problems
    assert "removed tool:read" in problems[0], problems
    assert any("--update" in line for line in problems), "the error carries the fix"
    budget = dict.fromkeys(BARE_ROLES, 100)
    rows = {("bare", role): 100 for role in BARE_ROLES}
    assert role_problems(budget, rows) == []
    over = role_problems(budget, {**rows, ("bare", "reader"): 101})
    assert over == ["bare reader prefix 101 > budget 100"], over
    gone = dict(rows)
    del gone[("bare", "worker")]
    assert any("bare worker" in line for line in role_problems(budget, gone)), "a lost row fails"
    unnamed = {role: 100 for role in BARE_ROLES if role != "leaf"}
    assert any("role_prefix.json has no leaf" in line for line in role_problems(unnamed, rows)), \
        "a role the baseline does not name fails, naming the fix"
    wide = {**rows, ("lane", "root"): 10**6, ("main", "reader"): 10**6}
    assert role_problems(budget, wide) == [], "a lane or main row never gates"
    parsed = role_rows("ROLE_PREFIX workspace=main role=leaf system=3 tools=4 total=7 claude=absent\n")
    assert parsed == {("main", "leaf"): 7}, parsed
    print("ok   request_budget selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    budget = json.loads((BASE / "request_budget.json").read_text())
    (system, tools, total), surface, rows = measure()
    lock = json.loads(LOCK.read_text()) if LOCK.exists() else None

    if "--update" in argv:
        (BASE / "request_budget.json").write_text(
            json.dumps({"system": system, "tools": tools, "total": total}, indent=2) + "\n")
        LOCK.write_text(json.dumps(surface, indent=2, sort_keys=True) + "\n")
        bare = {role: rows[("bare", role)] for role in BARE_ROLES if ("bare", role) in rows}
        ROLES.write_text(json.dumps(bare, indent=2, sort_keys=True) + "\n")
        print(f"ok   request_budget updated to {total} bytes, {len(surface)} surface keys")
        return 0

    errs = [f"{name} prefix {got} > budget {budget[name]}"
            for name, got in (("system", system), ("tools", tools), ("total", total))
            if got > budget[name]]
    errs += surface_problems(lock, surface)
    errs += role_problems(json.loads(ROLES.read_text()) if ROLES.exists() else {}, rows)
    fail(errs, f"request_budget ({total}/{budget['total']} bytes: system {system}, tools {tools})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
