#!/usr/bin/env python3
"""A change made on purpose stays made. A landed change file's `cut: <path> | <line> | <why>` records a
line removed on purpose, and `keep: <path> | <line> | <why>` one that must stay. A branch, by any author,
may not put a cut line back nor delete a kept one unless its own change file says
`revives: <change file> <path>` or `drops: <change file> <path>` beside the `decision:` that overturns it.

Incident: 7f097ff38 put back the plan cell's program record 3df4661cc had removed, 974fe064a's fold
dropped three of the owner's rules, and 7b3e430e9 put back a README block the owner cut. Each passed
every gate because nothing recorded the intent. The register is main's docs/changes/, read at the fork.

`unrecorded` is the half `just pr open` runs: a file that loses five or more substantive lines net
needs a `cut:` line, or `cut: none <path>`, in the branch's change file.
"""
import pathlib
import sys
from collections import defaultdict

sys.path[:0] = [str(pathlib.Path(__file__).parent), str(pathlib.Path(__file__).parents[1])]
from _common import ROOT, fail, fork, git, no_fork  # noqa: E402
from check_changes import CELL, files, held, parse, pending  # noqa: E402
from fixer_guard import CUT_AT, commits, substantive  # noqa: E402

FIXTURES = ROOT / "scripts/guardrails/fixtures/intent"


def anchors(fields, key):
    """(path, line) per `cut:` or `keep:` line; the line is every cell between the path and the why."""
    out = []
    for value in fields.get(key, []):
        cells = CELL.split(value)
        if len(cells) >= 3:
            out.append((cells[0].strip(), " | ".join(cells[1:-1]).strip()))
    return out


def net(diff):
    """{path: (added, deleted)} stripped, from a `git diff --unified=0` of the fork against the tree."""
    out = defaultdict(lambda: (set(), set()))
    for _, changed in commits("commit net\n" + diff):
        for path, (adds, dels) in changed.items():
            out[path][0].update(line.strip() for line in adds)
            out[path][1].update(line.strip() for line in dels)
    return out


def overturned(own, key, name, path, off=""):
    return any(f"{name} {path}" in (fields.get(key) or []) and (fields["decision"] or off == "decision")
               for fields in own) and off != "escape"


def hits(diff, register, own, off=""):
    """Each cut the diff puts back and each keep it deletes, with the change file that recorded it."""
    changed, out = net(diff), []
    for name, fields in register:
        for path, line in anchors(fields, "cut") if off != "cut" else ():
            adds, dels = changed.get(path, (set(), set()))
            if line in adds and line not in dels and not overturned(own, "revives", name, path, off):
                out.append(f"{path}: puts back `{line[:90]}`, which {name} cut on purpose; its change file "
                           f"needs `revives: {name} {path}` beside a `decision:` that overturns it")
        for path, line in anchors(fields, "keep") if off != "keep" else ():
            adds, dels = changed.get(path, (set(), set()))
            if line in dels and line not in adds and not overturned(own, "drops", name, path, off):
                out.append(f"{path}: deletes `{line[:90]}`, which {name} keeps on purpose; its change file "
                           f"needs `drops: {name} {path}` beside a `decision:` that overturns it")
    return out


def unrecorded(diff, own, off=""):
    """A file that loses CUT_AT or more substantive lines no other file gains, with no cut line for it."""
    changed = net(diff)
    moved = set().union(*(adds for adds, _ in changed.values())) if changed else set()
    named = {path for fields in own for path, _ in anchors(fields, "cut")}
    named |= {value[5:].strip() for fields in own for value in fields.get("cut", []) if value.startswith("none ") and off != "none"}
    return [f"{path}: {len(lost)} substantive line(s) deleted on net; record why with `cut: {path} | <line> | <why>`, "
            f"or `cut: none {path}`, in this branch's change file"
            for path, (adds, dels) in sorted(changed.items())
            if path not in named and len(lost := [line for line in dels if substantive(line) and line not in moved]) >= CUT_AT]


def branch_diff(base):
    return git("-c", "core.quotePath=false", "diff", "--unified=0", "--no-color", "--no-ext-diff", base).stdout


def selfcheck(off=""):
    plan, rules = "python/yi_runtime/src/yi/plan.py", ".ruler/082-skills.md"
    cut = (FIXTURES / "plan-cut.diff").read_text()
    revive, fold = (FIXTURES / "plan-revive.diff").read_text(), (FIXTURES / "rules-fold.diff").read_text()
    # The lines the authors would have recorded: one 3df4661cc deleted, three rules 974fe064a dropped.
    record = ('---\ncut: python/yi_runtime/src/yi/plan.py | await _send("program", {"cell_id": cell["id"], '
              '"source_ref": ref}, plan=plan_id, artifacts=[blob]) | a cell is no longer journaled as the plan\'s program\n'
              "keep: .ruler/082-skills.md | that falls to the bare-name list, or a description that needs the body to make "
              "sense, is a defect. | a skill must stand alone in the catalog\n"
              "keep: .ruler/082-skills.md | A feature-ledger row says it in a `taught:` clause beside `journey:`. | the ledger names where Yi is taught\n"
              "keep: .ruler/082-skills.md | run, a config key, a marker or file format it reads or writes, a convention. If Yi "
              "can reach it, | every reachable feature is taught\n---\nx\n")
    register = [("2026-10-29-plan-cell.md", parse(record)[0])]
    bad = []
    if anchors(register[0][1], "cut")[0][1] not in net(cut)[plan][1]:
        bad.append("the recorded cut is not a line 3df4661cc deleted")
    if len([h for h in hits(revive, register, [], off) if h.startswith(plan)]) != 1:
        bad.append("7f097ff38 putting back the program record is not refused")
    if len([h for h in hits(fold, register, [], off) if h.startswith(rules)]) != 3:
        bad.append("974fe064a dropping three kept rules is not refused three times")
    if hits(cut, register, [], off):
        bad.append("the cut itself is refused")
    lifted = parse("---\nrevives: 2026-10-29-plan-cell.md python/yi_runtime/src/yi/plan.py\n"
                   "decision: record the program again | the journal needs it | cut it again\n---\nx\n")[0]
    if hits(revive, register, [lifted], off):
        bad.append("a revive with its decision is still refused")
    if not hits(revive, register, [dict(lifted, decision=[])], off):
        bad.append("a revive with no decision lifts the cut")
    asked = lambda own: [h for h in unrecorded(cut, own, off) if h.startswith(plan)]
    if not asked([]):
        bad.append("3df4661cc's 12-line net deletion of plan.py is not asked for its cut line")
    if asked([parse(f"---\ncut: none {plan}\n---\nx\n")[0]]) or asked([register[0][1]]):
        bad.append("a `cut:` line, or `cut: none`, does not answer the net deletion it names")
    if not [h for h in unrecorded(fold, [], off) if h.startswith(rules)]:
        bad.append("974fe064a's fold, which reworded 17 rule lines away, is not asked for its cut line")
    return bad


def main():
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("cut", "keep", "escape", "decision", "none"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check off, so it refutes nothing")
        fail(errs, "intent selfcheck")
        return
    base = fork()
    if base is None:
        no_fork("intent")
    landed = held(base)
    register = [(name, parse(text)[0]) for name, text in files() if name in landed]
    own = [fields for _, fields in pending(base)]
    fail(hits(branch_diff(base), register, own), f"intent ({sum(len(f['cut']) + len(f['keep']) for _, f in register)} recorded)")


if __name__ == "__main__":
    main()
