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
The recorder adds `version:` and `decisions:` when it records the file."""
import pathlib, re, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail, fork, git, no_fork

DIR = "docs/changes"
NAME = re.compile(r"^\d{4}-\d{2}-\d{2}-[a-z0-9]+(-[a-z0-9]+)*\.md$")
KEYS = {"issue", "growth", "raise", "decision", "version", "decisions"}
GROWTH = re.compile(r"^\+(\d+)\s+\S")
RAISE = re.compile(r"^(tests|comments|over-cap|crate [a-z][a-z0-9-]*) \+(\d+)$")
VERSION = re.compile(r"^version:\s*(\S+)", re.M)
DECISIONS = "| id | decision | why | reversible via |"
# The selfcheck switches each check off in turn and must then fail for it (040).
CHECKS = ("header", "key", "decision", "raise", "growth", "twice", "prose", "name", "stamp", "exact", "rows", "version", "decisions")
OFF = set()


def parse(text):
    """(fields, errors). Fields: the header's values, `raise` summed per key, `body` the prose."""
    fields, errs = {"decision": [], "raise": {}}, []
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


def pending(base):
    """The change files this branch adds: in the working tree, absent at the fork. A file this gate
    refuses lends no raise and no memo, or a size gate run alone would be green on its numbers."""
    out = []
    for name, text in files():
        if name not in held(base) and not file_errors([(name, text)]):
            out.append((name, parse(text)[0]))
    return out


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


def cases():
    """Each case a refusal the gate owes; returns the ones that did not hold."""
    good = "---\nissue: Closes #974\ngrowth: +212 weighed X\nraise: tests +140, crate runtime +60\ndecision: a | b | c\n---\nProse.\n"
    log = "# Changelog\n\nprose\n\n| ver | date | change |\n|---|---|---|\n| 0.2.0 | d | two |\n"
    arch = f"version: 0.2.0  # x\n\n## Decisions\n\n{DECISIONS}\n|---|---|---|---|\n| D2 | a | b | c |\n\n## Next\n"
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
        "a name without its date": file_errors([("change-files.md", good)]),
        "a branch stamping its own version": file_errors([("2026-10-01-x.md", good.replace("---\nissue", "---\nversion: 0.9.0\nissue"))]),
        "a raise past the measured growth": raise_errors("tests", 110, 100, 50),
        "growth past the raise": raise_errors("tests", 160, 100, 50),
        "an added changelog row": edit("docs/CHANGELOG.md", "| 0.2.0 |", "| 0.3.0 | d | three |\n| 0.2.0 |"),
        "a bumped version": edit("docs/ARCHITECTURE.md", "0.2.0", "0.3.0"),
        "an added decision": edit("docs/ARCHITECTURE.md", "| D2 | a", "| D3 | z | y | x |\n| D2 | a"),
    }
    passed = {
        "a good file": file_errors([("2026-10-01-change-files.md", good)]),
        "a recorded file's stamp": file_errors([("2026-10-01-x.md", good.replace("---\nissue", "---\nversion: 0.9.0\nissue"))], {"2026-10-01-x.md"}),
        "a raise equal to the growth": raise_errors("tests", 150, 100, 50),
        "no raise and no growth": raise_errors("tests", 90, 100, 0),
        "changelog prose": edit("docs/CHANGELOG.md", "prose", "new prose"),
        "the version line's comment": edit("docs/ARCHITECTURE.md", "# x", "# y"),
        "a row in another table": view_edits(was, dict(was, **{"docs/ARCHITECTURE.md": arch + "| feature | row |\n"})),
    }
    bad = [f"not refused: {name}" for name, errs in refused.items() if not errs]
    bad += [f"refused: {name}: {errs}" for name, errs in passed.items() if errs]
    fields = parse(good)[0]
    if fields["raise"] != {"tests": 140, "crate runtime": 60} or fields["body"] != "Prose.":
        bad.append(f"a good file read as {fields}")
    if parse("---\nraise: tests +1\nraise: tests +2\n---\nx\n")[0]["raise"] != {"tests": 3}:
        bad.append("two raise lines did not sum")
    return bad


def fork_cases():
    """The size gates themselves, run in a scratch repo cut from main: growth goes red, the exact
    raise greens it, and a raise past the growth is refused. Fork blobs come through cat-file."""
    import shutil, subprocess, tempfile
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="yi-changes-"))
    run = lambda *a: subprocess.run(a, cwd=tmp, capture_output=True, text=True)
    git_ = lambda *a: run("git", "-c", "user.name=t", "-c", "user.email=t@t", *a)
    bad = []
    try:
        shutil.copytree(pathlib.Path(__file__).parent, tmp / "scripts/guardrails", ignore=shutil.ignore_patterns("baselines", "__pycache__"))
        (tmp / "crates/a/src").mkdir(parents=True)
        (tmp / "crates/a/tests").mkdir()
        (tmp / "crates/a/src/lib.rs").write_text("// one\nfn a() {}\n")
        (tmp / "crates/a/tests/t.rs").write_text("fn t() {}\n")
        git_("init", "-q", "-b", "main")
        git_("add", "-A")
        git_("commit", "-q", "-m", "seed")
        git_("checkout", "-q", "-b", "topic")
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
    finally:
        shutil.rmtree(tmp)
    return bad


def selfcheck():
    errs = cases()
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
    errs += view_edits({p: at(base, p) for p in views}, {p: (ROOT / p).read_text() for p in views})
    errs += tree_edits(base)
    fail(errs, f"changes ({len(pending(base))} pending in this branch, fork {base[:8]})")


if __name__ == "__main__":
    main()
