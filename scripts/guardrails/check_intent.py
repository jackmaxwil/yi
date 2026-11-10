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
import json
import pathlib
import re
import sys
from collections import defaultdict

sys.path[:0] = [str(pathlib.Path(__file__).parent), str(pathlib.Path(__file__).parents[1])]
from _common import ROOT, fail, fork, git, no_fork  # noqa: E402
from check_changes import CELL, files, held, parse, pending  # noqa: E402

FIXTURES = ROOT / "scripts/guardrails/fixtures/intent"
# A finding about deleted text sits beside where it was: #2062's high asking for the cut block back was 3 lines below it.
NEAR = 3
CUT_AT = 5


def substantive(line):
    """A blank, a brace or a short comment comes and goes in any edit; a line of content does not."""
    s = line.strip()
    return len(s) >= 40 or (len(s) >= 25 and not s.startswith(("//", "#")))


def commits(log):
    """(sha, {path: (added, deleted)}) per commit. A deleted `-- x` line prints as `--- x`, so file
    headers are read only between `diff --git` and the first hunk."""
    out, files, header, old, path = [], None, False, None, None
    for line in log.splitlines():
        if line.startswith("commit "):
            files, header, path = defaultdict(lambda: ([], [])), False, None
            out.append((line.split()[1], files))
        elif line.startswith("diff --git "):
            header, old, path = True, None, None
        elif header and line.startswith("--- "):
            old = line[6:] if line.startswith("--- a/") else None
        elif header and line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else old
        elif line.startswith("@@"):
            header = False
        elif not header and path and files is not None and line[:1] in ("+", "-"):
            files[path][line[0] == "-"].append(line[1:])
    return out


def anchors(fields, key, off=""):
    """(path, line) per `cut:` or `keep:` line; the line is every cell between the path and the why, `\\|` read as `|`."""
    out = []
    for value in fields.get(key, []):
        cells = CELL.split(value)
        if len(cells) >= 3:
            line = " | ".join(cells[1:-1]).strip()
            out.append((cells[0].strip(), line if off == "unescape" else line.replace("\\|", "|")))
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


def records_in(root):
    """(change file, kind, path, line) for every cut and keep in a checkout's change files."""
    out = []
    for change in sorted((pathlib.Path(root) / "docs/changes").glob("*.md")):
        fields = parse(change.read_text())[0]
        out += [(change.name, kind, path, line) for kind in ("cut", "keep") for path, line in anchors(fields, kind)]
    return out


def rows(diff):
    """(path, sign, head-side line, text) per added, deleted or context row of a unified diff; a deleted
    row carries the head line it sat before."""
    path, new = None, None
    for row in diff.splitlines():
        if row.startswith("+++ "):
            path, new = row[6:] if row.startswith("+++ b/") else None, None
        elif (hunk := re.match(r"@@ -\d+(?:,\d+)? \+(\d+)", row)):
            new = int(hunk.group(1))
        elif path and new is not None and row[:1] in ("+", "-", " ") and not row.startswith("--- "):
            yield path, row[0], new, row[1:]
            new += row[0] != "-"


def deleted_at(diff, path, line):
    return [at for where, sign, at, text in rows(diff) if where == path and sign == "-" and text.strip() == line]


def contrary(finding, records, diff, off=""):
    """Why a review finding asks to undo a recorded intent, or None: its quote is a kept line, or it sits
    within NEAR lines of where the PR deleted a cut line."""
    for name, kind, path, line in records:
        if path != finding.get("path"):
            continue
        if kind == "keep" and off != "keep" and (finding.get("quote") or "").strip() == line:
            return f"`{path}:{finding.get('line')}` would delete `{line[:80]}`, which {name} keeps on purpose"
        if kind == "cut" and off != "cut" and any(abs(at - (finding.get("line") or 0)) <= (0 if off == "near" else NEAR)
                                                  for at in deleted_at(diff, path, line)):
            return f"`{path}:{finding.get('line')}` would put back `{line[:80]}`, which {name} cut on purpose"
    return None


def intent_block(records):
    if not records:
        return ""
    rows = [f"- {kind} by {name} in {path}: {line}" for name, kind, path, line in records]
    return ("<recorded-intent>\nThe owner removed or kept these lines on purpose. A finding that asks to put a cut line "
            "back, or to remove a kept line, is dropped before the round posts.\n" + "\n".join(rows) + "\n</recorded-intent>\n")


def review_selfcheck(off=""):
    """#2062's round 1, posted on e1b555564, against the cut and keep its author would have recorded."""
    findings = json.loads((FIXTURES / "pr2062-round1.json").read_text())
    diff = (FIXTURES / "readme-pr.diff").read_text()
    records = [("2026-11-06-readme-guide.md", "cut", "README.md", "It checks the release's sha256 and the owner's key, "
                "then installs `yi` to `~/.local/bin` and its"), ("2026-11-06-readme-guide.md", "keep", "README.md", "flowchart TB")]
    dropped = {f["line"] for f in findings if contrary(f, records, diff, off)}
    if dropped != {21, 128}:
        return [f"#2062's round 1 dropped the findings at lines {sorted(dropped)}, not 21 (restore the cut block) and 128 (delete the kept chart)"]
    return []


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
    if anchors(parse("---\ncut: docs/a.md | `a \\| b` | a table cell\n---\nx\n")[0], "cut", off) != [("docs/a.md", "`a | b`")]:
        bad.append("a recorded line written with `\\|` keeps its backslash, so it never matches the deleted line")
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
        errs += review_selfcheck()
        for off in ("cut", "keep", "escape", "decision", "none", "unescape"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check off, so it refutes nothing")
        for off in ("cut", "keep", "near"):
            if not review_selfcheck(off):
                errs.append(f"the review selfcheck passes with the {off} match off, so it refutes nothing")
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
