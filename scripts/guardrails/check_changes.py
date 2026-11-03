#!/usr/bin/env python3
"""A change is recorded in its own file, never in a shared counter.

A PR writes `docs/changes/<yyyy-mm-dd>-<slug>.md`: a header of `key: value` lines between `---`
fences, then the changelog prose. The version, the decision numbers, the changelog row, the
decision-table row and the ADR are views the recorder writes on main after the merge, so a PR
that edits one is refused here, as is one that edits a change file main already holds.
Incident: on 2026-09-30 all 9 open PRs that conflicted with main collided on the version line,
the changelog top and the size ceilings, and the forge cannot merge a shared counter.

Header keys, each optional:
  issue: Closes #974            the issue this change finishes or is part of
  growth: +212 <memo>           net src growth past the free band, and what was weighed for deletion
  raise: tests +140, comments +12, over-cap +1, crate runtime +60
  decision: <decision> | <why> | <reversible via>      repeatable; numbered by the recorder
  ledger+: <feature> | <clause>                         append `; <clause>` to the row's note
  ledger~: <feature> | <old> | <new>                    replace text occurring once in the row
  ledger=: <feature> | <§> | <status> | <V> | <C> | <note>      a new row, after the last
  ledger-: <feature>                                    delete the row
Cells split on an unescaped ` | `; a literal pipe is `\\|`, as the table writes it. Incident: 43 of
100 conflicted main merges touched a ledger row, a 2-4 KB line every feature appends to.
scripts/record_changes.py adds `version:` and `decisions:` when it records the file, and its own
diff, exactly what it writes from the fork's tree, is the one view edit a branch may carry."""
import functools, os, pathlib, re, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail, fork, git, no_fork, row_key, scratch_repo, table_rows

DIR = "docs/changes"
NAME = re.compile(r"^\d{4}-\d{2}-\d{2}-[a-z0-9]+(-[a-z0-9]+)*\.md$")
LEDGER, LEDGER_OPS = "Feature ledger", {"ledger+": 2, "ledger~": 3, "ledger=": 6, "ledger-": 1}
CELL = re.compile(r"(?<!\\) \| ")
KEYS = {"issue", "growth", "raise", "decision", "version", "decisions", *LEDGER_OPS}
GROWTH = re.compile(r"^\+(\d+)\s+\S")
RAISE = re.compile(r"^(tests|comments|over-cap|crate [a-z][a-z0-9-]*) \+(\d+)$")
VERSION = re.compile(r"^version:\s*(\S+)", re.M)
DECISIONS = "| id | decision | why | reversible via |"
# The selfcheck switches each check off in turn and must then fail for it (040).
CHECKS = ("header", "key", "decision", "raise", "growth", "twice", "prose", "name", "stamp", "exact", "rows", "version", "decisions",
          "ledger-edit", "ledger-apply", "ledger-ops")
OFF = set()


def parse(text):
    """(fields, errors). Fields: the header's values, `raise` summed per key, `body` the prose."""
    fields, errs = {"decision": [], "raise": {}, "ledger": []}, []
    head, sep, body = text[4:].partition("\n---\n")
    if text[:4] != "---\n" or not sep:
        if "header" not in OFF:
            return fields, ["no header: the file opens with a `---` line, its `key: value` lines, then `---`"]
        head, body = "", text
    for line in filter(str.strip, head.splitlines()):
        key, colon, value = line.partition(":")
        key, value = key.strip(), value.strip()
        if "key" not in OFF and (not colon or key not in KEYS):
            errs.append(f"header line {line!r}: the keys are {', '.join(sorted(KEYS))}")
        elif key == "decision":
            if "decision" not in OFF and (value.count("|") != 2 or not all(cell.strip() for cell in value.split("|"))):
                errs.append("a decision is three cells: <decision> | <why> | <reversible via>")
            fields["decision"].append(value)
        elif key in LEDGER_OPS:
            cells = [cell.strip() for cell in CELL.split(value)]
            if len(cells) == LEDGER_OPS[key] and all(cells):
                fields["ledger"].append((key, cells))
            elif "ledger-ops" not in OFF:
                errs.append(f"{key} takes {LEDGER_OPS[key]} cell(s) split by ` | `, and {line!r} has {len(cells)}")
        elif key == "raise":
            for part in filter(None, (p.strip() for p in value.split(","))):
                found = RAISE.match(part)
                if not found and "raise" not in OFF:
                    errs.append(f"raise {part!r}: write `tests +N`, `comments +N`, `over-cap +N` or `crate <name> +N`")
                elif found:
                    fields["raise"][found.group(1)] = fields["raise"].get(found.group(1), 0) + int(found.group(2))
        elif key == "growth" and not GROWTH.match(value) and "growth" not in OFF:
            errs.append("growth reads `+N <what was weighed for deletion>`")
        elif key in fields and key != "decision" and "twice" not in OFF:
            errs.append(f"{key} appears twice")
        else:
            fields[key] = value
    fields["body"] = " ".join(body.split())
    if not fields["body"] and "prose" not in OFF:
        errs.append("no changelog prose after the header")
    return fields, errs


def at(rev, path):
    shown = git("show", f"{rev}:{path}")
    return shown.stdout if shown.returncode == 0 else None


@functools.cache
def pending(base):
    """The change files this branch adds: in the working tree, absent at the fork. A file this gate
    refuses lends no raise and no memo, or a size gate run alone would be green on its numbers.
    Incident: an ls-tree per file per caller made check_crate_size.py take 68 s on 187 change files."""
    fork_names = held(base)
    return tuple((name, parse(text)[0]) for name, text in files()
                 if name not in fork_names and not file_errors([(name, text)]))


def held(base):
    return set(git("ls-tree", "--name-only", f"{base}:{DIR}").stdout.split())


def files():
    return [(p.name, p.read_text()) for p in sorted((ROOT / DIR).glob("*.md"))] if (ROOT / DIR).is_dir() else []


def raised(base, key):
    return sum(fields["raise"].get(key, 0) for _, fields in pending(base))


def raise_errors(key, now, was, declared):
    """The raise is the measured growth: short of it the gate is red, and past it a ceiling would be
    lifted by a number nobody measured."""
    grew = max(now - was, 0)
    if now > was + declared:
        return [f"{key} {now} > {was + declared} (fork {was}); a change file here says `raise: {key} +{grew}`"]
    if declared > grew and "exact" not in OFF:
        return [f"a change file here raises {key} +{declared} but the fork measures {now - was:+d}; "
                + (f"write `raise: {key} +{grew}`" if grew else "drop the raise")]
    return []


def table(text, header):
    """The rows under a table whose header line is `header`: the decision log's rows."""
    lines, out, inside = text.splitlines(), [], False
    for line in lines:
        if line.startswith(header):
            inside = True
        elif inside and line.startswith("|"):
            out.append(line)
        elif inside:
            break
    return out


def changelog_rows(text):
    return [line for line in text.splitlines() if re.match(r"^\| \d+\.\d+\.\d+ \|", line)]


def apply_ledger(arch, lines):
    """(arch, errors) after each (op, cells) in turn; a line that does not apply changes nothing."""
    errs = []
    for op, cells in lines:
        rows = {row_key(row): row for row in table_rows(arch, LEDGER)}
        row = rows.get(cells[0])
        if op == "ledger=":
            if row is None:
                last = list(rows.values())[-1]
                arch = arch.replace(last, f"{last}\n| {' | '.join(cells)} |", 1)
            else:
                errs.append(f"{op}: `{cells[0]}` already has a row")
        elif row is None:
            errs.append(f"{op}: no feature-ledger row named `{cells[0]}`")
        elif op == "ledger-":
            arch = arch.replace(row + "\n", "", 1)
        elif op == "ledger+":
            arch = arch.replace(row, f"{row[:-2]}; {cells[1]} |", 1)
        elif row.count(cells[1]) == 1:
            arch = arch.replace(row, row.replace(cells[1], cells[2]), 1)
        elif (parts := CELL.split(row[2:-2])).count(cells[1]) == 1:
            parts[parts.index(cells[1])] = cells[2]
            arch = arch.replace(row, f"| {' | '.join(parts)} |", 1)
        else:
            errs.append(f"{op}: `{cells[1]}` occurs {row.count(cells[1])} times in `{cells[0]}`, not once")
    return arch, [] if "ledger-apply" in OFF else errs


def unapplied_ledger(arch):
    """(arch, errors) with every unstamped file's lines applied in the recorder's order, the one view
    every reader judges: stamped and applied happen in one recorder commit."""
    sys.path.insert(0, str(ROOT / "scripts"))
    import record_changes
    written = [(name, text) for name, text in files() if parse(text)[0]["ledger"]]
    if not written:
        return arch, []
    errs = []
    for name, text, _ in record_changes.pending(ROOT, "HEAD", written):
        arch, found = apply_ledger(arch, parse(text)[0]["ledger"])
        errs += [f"{DIR}/{name}: {e}" for e in found]
    return arch, errs


def cell_edit(key, row, old, new):
    """`old` to `new` as a `~`, cut small so a second PR on the cell still applies, or as its whole
    cell when a status also occurs in the note."""
    p = len(os.path.commonprefix([old, new]))
    s = len(os.path.commonprefix([old[p:][::-1], new[p:][::-1]]))
    cut = lambda text: text[p:len(text) - s]
    while s and (cut(old)[-1:].isspace() or cut(new)[-1:].isspace()):
        s -= 1
    while p and (row.count(cut(old)) != 1 or not cut(new) or cut(old)[:1].isspace() or cut(new)[:1].isspace()):
        p -= 1
    return f"ledger~: {key} | {cut(old)} | {cut(new)}" if row.count(cut(old)) == 1 and cut(new) else f"ledger~: {key} | {old} | {new}"


def row_edit(key, old, row):
    """The lines that turn `old` into `row`, proven by applying them; a delete and a new row, which
    moves the row to the table's end, only when they do not reach it."""
    if row.startswith(f"{old[:-2]}; "):
        return f"ledger+: {key} | {row[len(old):-2]}"
    was, now = CELL.split(old[2:-2]), CELL.split(row[2:-2])
    lines = "\n".join(cell_edit(key, old, a, b) for a, b in zip(was, now) if a != b)
    read, errs = parse(f"---\n{lines}\n---\nx\n")
    done, found = apply_ledger(f"## {LEDGER}\n\n| f |\n|---|\n{old}\n", read["ledger"])
    if len(was) == len(now) and not errs and not found and table_rows(done, LEDGER) == [row]:
        return lines
    return f"ledger-: {key}\nledger=: {row[2:-2]}"


def ledger_lines(was, now):
    """The change-file lines that turn `was`'s table into `now`'s."""
    old, new = ({row_key(row): row for row in table_rows(text, LEDGER)} for text in (was, now))
    out = [f"ledger-: {key}" for key in old if key not in new]
    for key, row in new.items():
        if key not in old:
            out.append(f"ledger=: {row[2:-2]}")
        elif old[key] != row:
            out.append(row_edit(key, old[key], row))
    return out




def view_edits(was, now):
    """What a PR changed in the recorder's views. `was` and `now` map path -> text (None: absent)."""
    errs = []
    if "rows" not in OFF and changelog_rows(was["docs/CHANGELOG.md"] or "") != changelog_rows(now["docs/CHANGELOG.md"] or ""):
        errs.append("docs/CHANGELOG.md: a changelog row changed; the recorder writes rows from docs/changes/")
    arch_was, arch_now = was["docs/ARCHITECTURE.md"] or "", now["docs/ARCHITECTURE.md"] or ""
    if "version" not in OFF and VERSION.findall(arch_was) != VERSION.findall(arch_now):
        errs.append("docs/ARCHITECTURE.md: the `version:` line changed; the recorder numbers versions")
    if "decisions" not in OFF and table(arch_was, DECISIONS) != table(arch_now, DECISIONS):
        errs.append("docs/ARCHITECTURE.md: a decision row changed; write `decision:` in a change file")
    if "ledger-edit" not in OFF and table_rows(arch_was, LEDGER) != table_rows(arch_now, LEDGER):
        lines = "\n".join(f"  {line}" for line in ledger_lines(arch_was, arch_now))
        errs.append(f"docs/ARCHITECTURE.md: a feature-ledger row changed; restore the table from the fork and add to a change file:\n{lines}")
    return errs


def tree_edits(base):
    """ADR files and change files main already holds, edited or deleted here."""
    errs = []
    for path in git("diff", "--name-only", "--no-renames", base, "--", "docs/solutions/adr", DIR).stdout.splitlines():
        exists_at_fork = at(base, path) is not None
        if path.startswith("docs/solutions/adr/"):
            errs.append(f"{path}: ADRs are rendered from the decision log by the recorder")
        elif exists_at_fork:
            errs.append(f"{path}: main holds this change file; add a new one instead")
    return errs


def recorded(base):
    """The recorder's own diff is the one a branch may carry into the views: every path it writes
    from the fork's tree holds exactly that text here, and nothing else under those paths changed."""
    sys.path.insert(0, str(ROOT / "scripts"))
    import record_changes
    writes, fork_names = record_changes.expected_at(ROOT, base), held(base)
    changed = {p for p in git("diff", "--name-only", "--no-renames", base, "--", "docs/CHANGELOG.md", "docs/ARCHITECTURE.md",
                              "docs/solutions", DIR).stdout.split() if not p.startswith(f"{DIR}/") or p[len(DIR) + 1:] in fork_names}
    return bool(writes) and changed <= writes.keys() and all(
        (ROOT / p).is_file() and (ROOT / p).read_text() == text for p, text in writes.items())


def file_errors(names_texts, recorded=()):
    errs = []
    for name, text in names_texts:
        fields, found = parse(text)
        if not NAME.match(name) and "name" not in OFF:
            errs.append(f"{DIR}/{name}: name it <yyyy-mm-dd>-<slug>.md, lowercase words joined by '-'")
        if name not in recorded and ("version" in fields or "decisions" in fields) and "stamp" not in OFF:
            errs.append(f"{DIR}/{name}: `version:` and `decisions:` are the recorder's; a branch never writes them")
        errs += [f"{DIR}/{name}: {e}" for e in found]
    return errs


LEDGER_FIXTURE = (f"## {LEDGER}\n\n| feature | § | status | V | C | note |\n|---|---|---|---|---|---|\n"
                  "| one | 1 | core | H | M | first core |\n\n| two – § | 2 | core | H | L | has \\| pipe |\n\n## Next\n")
GOOD = "---\nissue: Closes #974\ngrowth: +212 weighed X\nraise: tests +140, crate runtime +60\ndecision: a | b | c\n---\nProse.\n"


def cases():
    """Each case a refusal the gate owes; returns the ones that did not hold."""
    log = "# Changelog\n\nprose\n\n| ver | date | change |\n|---|---|---|\n| 0.2.0 | d | two |\n"
    arch = f"version: 0.2.0  # x\n\n## Decisions\n\n{DECISIONS}\n|---|---|---|---|\n| D2 | a | b | c |\n\n{LEDGER_FIXTURE}"
    was = {"docs/CHANGELOG.md": log, "docs/ARCHITECTURE.md": arch}
    edit = lambda path, a, b: view_edits(was, dict(was, **{path: was[path].replace(a, b)}))
    refused = {
        "a file without its header": parse("Prose only\n")[1],
        "an unknown key": parse("---\nowner: me\n---\nx\n")[1],
        "a decision short a cell": parse("---\ndecision: a | b\n---\nx\n")[1],
        "a raise of no ceiling": parse("---\nraise: speed +3\n---\nx\n")[1],
        "a growth memo that weighs nothing": parse("---\ngrowth: +212\n---\nx\n")[1],
        "a key twice": parse("---\nissue: #1\nissue: #2\n---\nx\n")[1],
        "a file without prose": parse("---\nissue: Closes #1\n---\n\n")[1],
        "a name without its date": file_errors([("change-files.md", GOOD)]),
        "a branch stamping its own version": file_errors([("2026-10-01-x.md", GOOD.replace("---\nissue", "---\nversion: 0.9.0\nissue"))]),
        "a raise past the measured growth": raise_errors("tests", 110, 100, 50),
        "growth past the raise": raise_errors("tests", 160, 100, 50),
        "an added changelog row": edit("docs/CHANGELOG.md", "| 0.2.0 |", "| 0.3.0 | d | three |\n| 0.2.0 |"),
        "a bumped version": edit("docs/ARCHITECTURE.md", "0.2.0", "0.3.0"),
        "an added decision": edit("docs/ARCHITECTURE.md", "| D2 | a", "| D3 | z | y | x |\n| D2 | a"),
        "an edited ledger row": edit("docs/ARCHITECTURE.md", "first", "changed"),
        "an old PR's ledger edit": view_edits(was, dict(was, **{"docs/ARCHITECTURE.md": arch.replace("first", "changed")})),
        "a line short a cell": parse("---\nledger~: one | old\n---\nx\n")[1],
        "a line one cell over": parse("---\nledger+: one | a | b\n---\nx\n")[1],
        "a new row one cell short": parse("---\nledger=: new | 1 | core | H | M\n---\nx\n")[1],
        "a new row one cell over": parse("---\nledger=: new | 1 | core | H | M | n | o\n---\nx\n")[1],
        "old text the row does not hold": apply_ledger(LEDGER_FIXTURE, [("ledger~", ["one", "absent", "x"])])[1],
        "old text the row holds twice": apply_ledger(LEDGER_FIXTURE, [("ledger~", ["one", "co", "x"])])[1],
        "a new row on an existing key": apply_ledger(LEDGER_FIXTURE, [("ledger=", ["one", "1", "core", "H", "M", "n"])])[1],
        "a clause on a missing key": apply_ledger(LEDGER_FIXTURE, [("ledger+", ["ghost", "x"])])[1],
        "a delete of a missing key": apply_ledger(LEDGER_FIXTURE, [("ledger-", ["ghost"])])[1],
    }
    passed = {
        "a good file": file_errors([("2026-10-01-change-files.md", GOOD)]),
        "a recorded file's stamp": file_errors([("2026-10-01-x.md", GOOD.replace("---\nissue", "---\nversion: 0.9.0\nissue"))], {"2026-10-01-x.md"}),
        "a raise equal to the growth": raise_errors("tests", 150, 100, 50),
        "no raise and no growth": raise_errors("tests", 90, 100, 0),
        "changelog prose": edit("docs/CHANGELOG.md", "prose", "new prose"),
        "the version line's comment": edit("docs/ARCHITECTURE.md", "# x", "# y"),
        "a row in another table": view_edits(was, dict(was, **{"docs/ARCHITECTURE.md": arch + "| feature | row |\n"})),
    }
    bad = [f"not refused: {name}" for name, errs in refused.items() if not errs]
    bad += [f"refused: {name}: {errs}" for name, errs in passed.items() if errs]
    fields = parse(GOOD)[0]
    if fields["raise"] != {"tests": 140, "crate runtime": 60} or fields["body"] != "Prose.":
        bad.append(f"a good file read as {fields}")
    if parse("---\nraise: tests +1\nraise: tests +2\n---\nx\n")[0]["raise"] != {"tests": 3}:
        bad.append("two raise lines did not sum")
    return bad


def ledger_cases():
    """Each op on the rows it is for, below a blank line in the table and through an escaped pipe, and
    the two real edits of the 3 KB context row that #1890 and #1870 made, as lines."""
    bad = []
    lines = parse("---\nledger~: two – § | has \\| pipe | no pipe\nledger+: two – § | c \\| d\nledger=: new | 3 | gated | M | L | n\n---\nx\n")
    if lines[1] or len(lines[0]["ledger"]) != 3:
        bad.append(f"a line with an escaped pipe read as {lines}")
    now, errs = apply_ledger(LEDGER_FIXTURE, lines[0]["ledger"])
    want = ["| one | 1 | core | H | M | first core |", "| two – § | 2 | core | H | L | no pipe; c \\| d |", "| new | 3 | gated | M | L | n |"]
    if errs or table_rows(now, LEDGER) != want:
        bad.append(f"three ops on the fixture read {table_rows(now, LEDGER)}: {errs}")
    if table_rows(apply_ledger(LEDGER_FIXTURE, [("ledger-", ["one"])])[0], LEDGER) != table_rows(LEDGER_FIXTURE, LEDGER)[1:]:
        bad.append("a deleted row is still in the table")
    for name, after in {"a clause": LEDGER_FIXTURE.replace("first core", "first core; more"),
                        "a mid-row insert": LEDGER_FIXTURE.replace("first core", "first, then core"),
                        "a status": LEDGER_FIXTURE.replace("1 | core | H", "1 | gated | H"),
                        "a deletion": LEDGER_FIXTURE.replace("first core", "first"),
                        "a cut across cells": LEDGER_FIXTURE.replace("H | M | first", "L | L | second"),
                        "a new row": LEDGER_FIXTURE.replace("\n\n## Next", "\n| new | 3 | core | H | M | n |\n\n## Next"),
                        "a deleted row": LEDGER_FIXTURE.replace("| one | 1 | core | H | M | first core |\n", "")}.items():
        text = "\n".join(ledger_lines(LEDGER_FIXTURE, after))
        read, errs = parse(f"---\n{text}\n---\nx\n")
        if errs or table_rows(apply_ledger(LEDGER_FIXTURE, read["ledger"])[0], LEDGER) != table_rows(after, LEDGER):
            bad.append(f"the lines for {name} do not reproduce it in place: {text!r} {errs}")
    swapped = LEDGER_FIXTURE.replace("H | M", "M | L")
    read, errs = parse("---\n" + "\n".join(ledger_lines(LEDGER_FIXTURE, swapped)) + "\n---\nx\n")
    now, found = apply_ledger(LEDGER_FIXTURE, read["ledger"])
    if errs or found or sorted(table_rows(now, LEDGER)) != sorted(table_rows(swapped, LEDGER)):
        bad.append(f"lines on two cells that interact read {read['ledger']}: {errs} {found}")
    twin = LEDGER_FIXTURE.replace("H | M", "H | H")
    text = "\n".join(ledger_lines(twin, twin.replace("H | H", "L | H")))
    if not text.startswith("ledger-: one\nledger=: one"):
        bad.append(f"a cell spelled twice in its row read {text!r}, which applies to the wrong cell")
    arch = "docs/ARCHITECTURE.md"
    history = [(at(f"{m}^", arch), at(tip, arch)) for m, tip in (("aa93a221b^{commit}^0", "398b811c7"), ("c42e30f01^{commit}^0", "3e410a078"))]
    if any(None in pair for pair in history):
        return bad + ["the #1890 and #1870 commits are missing; fetch main's full history"]
    return bad + real_row_cases(history)


def real_row_cases(history):
    arch = history[0][0]
    for was, now in history:
        read, errs = parse("---\n" + "\n".join(ledger_lines(was, now)) + "\n---\nx\n")
        arch, found = apply_ledger(arch, read["ledger"])
        if errs or found:
            return [f"a real row edit does not apply as lines: {errs} {found}"]
    row = {row_key(r): r for r in table_rows(arch, LEDGER)}["context P1–P19; prefix-aligned P7"]
    held = all(text in row for text in ("a session runs one compaction at a time", "the retention floor keeps only the words the user typed"))
    return [] if held else [f"the real row lost a PR's clause: {row[-300:]}"]


def fork_cases():
    """The size gates themselves, run in a scratch repo cut from main: growth goes red, the exact
    raise greens it, and a raise past the growth is refused. Fork blobs come through cat-file."""
    bad = []
    with scratch_repo("yi-changes-", {"crates/a/src/lib.rs": "// one\nfn a() {}\n", "crates/a/tests/t.rs": "fn t() {}\n"}) as (tmp, run):
        with open(tmp / "crates/a/src/lib.rs", "a") as f:
            f.write("// two\n// three\n// four\n" + "".join(f"fn g{i}() {{}}\n" for i in range(200)))
        (tmp / "crates/a/tests/t.rs").write_text("fn t() {}\nfn u() {}\n")
        gates = ("crate_size", "test_size", "comments", "growth")
        verdict = lambda: {g: run(sys.executable, f"scripts/guardrails/check_{g}.py").returncode for g in gates}
        if verdict() != dict.fromkeys(gates, 1):
            bad.append(f"growth with no change file read {verdict()}, not all red")
        (tmp / DIR).mkdir(parents=True)
        change = "---\ngrowth: +203 weighed the fixture\nraise: crate a +203, tests +1, comments +3, over-cap +1\n---\nGrow.\n"
        (tmp / DIR / "2026-10-01-grow.md").write_text(change)
        if verdict() != dict.fromkeys(gates, 0):
            bad.append(f"exact raises read {verdict()}, not all green")
        (tmp / DIR / "2026-10-01-grow.md").write_text(change.replace("tests +1", "tests +9"))
        if verdict()["test_size"] != 1:
            bad.append("a raise past the measured growth was accepted")
        (tmp / DIR / "2026-10-01-grow.md").write_text(change.replace("---\nGrow.", "---\n"))
        if verdict()["test_size"] != 1:
            bad.append("a change file this gate refuses still lent its raise")
    return bad


def scan_cases():
    """Every size gate asks pending() once per crate or key; the fork's listing is read once a process."""
    global git, files
    calls, real = [], (git, files)
    listing = lambda *a: calls.append(a) or type("Run", (), {"stdout": "", "returncode": 0})()
    git, files = listing, lambda: [(f"2026-10-{1 + i % 28:02d}-change-{i}.md", GOOD) for i in range(187)]
    pending.cache_clear()
    try:
        asks = [raised("fork", key) for key in ("tests", "comments", "over-cap", "crate runtime")]
    finally:
        git, files = real
        pending.cache_clear()
    bad = [] if len(calls) == 1 else [f"pending() ran git {len(calls)} times for {len(asks)} asks over 187 change files, not once"]
    return bad + ([] if asks == [140 * 187, 0, 0, 60 * 187] else [f"187 change files summed to {asks}"])


def selfcheck():
    errs = cases() + scan_cases() + ledger_cases()
    for check in CHECKS:
        OFF.clear()
        OFF.add(check)
        if not cases():
            errs.append(f"selfcheck passes with the {check} check off, so it refutes nothing")
    OFF.clear()
    fail(errs + fork_cases(), "changes selfcheck")


def main():
    if "--selfcheck" in sys.argv:
        selfcheck()
        return
    base = fork()
    if base is None:
        no_fork("changes")
    views = ("docs/CHANGELOG.md", "docs/ARCHITECTURE.md")
    errs = file_errors(files(), held(base))
    now = {p: (ROOT / p).read_text() for p in views}
    edits = view_edits({p: at(base, p) for p in views}, now) + tree_edits(base)
    errs += edits if edits and not recorded(base) else []
    errs += unapplied_ledger(now["docs/ARCHITECTURE.md"])[1]
    fail(errs, f"changes ({len(pending(base))} pending in this branch, fork {base[:8]})")


if __name__ == "__main__":
    main()
