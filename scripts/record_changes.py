#!/usr/bin/env python3
"""Record landed change files into the views main keeps: the version, the changelog, the decision log.

    record_changes.py               record every unstamped file in docs/changes, in landing order
    record_changes.py --dry-run     print the plan and write nothing
    record_changes.py --backfill    also record a backlog: files that landed before any was recorded
    record_changes.py --selfcheck   run the recorder over real change files in a scratch repo

Per file, in the order main's first-parent history added them (name order within one commit): the
minor version in docs/ARCHITECTURE.md goes up by one, its CHANGELOG row goes on top, each `decision:`
becomes the next D-row with its ADR and index line, and the file is stamped `version:` and
`decisions:`, which check_changes.py refuses a branch to write. A stamped file is never recorded again.
"""
import os, pathlib, re, shutil, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "scripts"), str(ROOT / "scripts/guardrails")]
import adr
from check_changes import DECISIONS, DIR, VERSION, parse, table

ARCH, LOG, INDEX = "docs/ARCHITECTURE.md", "docs/CHANGELOG.md", "docs/solutions/README.md"
LOG_HEADER = "| ver | date | change |"
ROW_ID = re.compile(r"^\| D(\d+) \|")


def git(root, *args):
    done = subprocess.run(("git", "-C", str(root)) + args, capture_output=True, text=True, check=False)
    if done.returncode:
        sys.exit(f"record: git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout


def landed(root, rev):
    """{name: (position, date)}: the first-parent commit on `rev` that added each change file."""
    out, date = {}, None
    log = git(root, "log", "--first-parent", "--reverse", "--diff-filter=A", "--no-renames", "--name-only",
              "--format=@%cs", rev, "--", DIR)
    for line in log.splitlines():
        if line.startswith("@"):
            date = line[1:]
        elif line.strip():
            out.setdefault(pathlib.PurePath(line).name, (len(out), date))
    return out


def state(root, rev=None):
    """(views, change files as (name, text)) from the working tree, or from `rev`'s tree."""
    if rev is None:
        read = lambda path: (root / path).read_text()
        names = sorted(p.name for p in (root / DIR).glob("*.md"))
    else:
        read = lambda path: git(root, "show", f"{rev}:{path}")
        names = sorted(git(root, "ls-tree", "--name-only", f"{rev}:{DIR}").split())
    return {p: read(p) for p in (ARCH, LOG, INDEX)}, [(n, read(f"{DIR}/{n}")) for n in names]


def pending(root, rev, changes):
    """The unstamped files as (name, text, date), oldest landing first; an unlanded file goes last."""
    order = landed(root, rev)
    todo = [(n, t) for n, t in changes if "version" not in parse(t)[0]]
    todo.sort(key=lambda nt: (order.get(nt[0], (len(order),))[0], nt[0]))
    return [(n, t, order[n][1] if n in order else n[:10]) for n, t in todo]


def insert_row(text, header, row):
    """A row on top of the table under `header`, below its `|---|` line: both tables run newest first."""
    lines = text.split("\n")
    at = next((i for i, line in enumerate(lines) if line.startswith(header)), None)
    if at is None:
        sys.exit(f"record: no table headed {header!r}")
    lines.insert(at + 2, row)
    return "\n".join(lines)


def stamp(text, version, numbers):
    head = f"---\nversion: {version}\n" + (f"decisions: {', '.join(numbers)}\n" if numbers else "")
    return head + text.removeprefix("---\n")


def record(views, todo):
    """(views, writes, plan) after recording `todo` in order. Pure: texts in, texts out."""
    arch, log, index = views[ARCH], views[LOG], views[INDEX]
    writes, plan = {}, []
    for name, text, date in todo:
        fields = parse(text)[0]
        major, minor, _ = VERSION.search(arch).group(1).split(".")
        version = f"{major}.{int(minor) + 1}.0"
        arch = re.sub(r"^(version:\s*)\S+", lambda m: m.group(1) + version, arch, count=1, flags=re.M)
        last = max((int(m.group(1)) for row in table(arch, DECISIONS) if (m := ROW_ID.match(row))), default=0)
        numbers = []
        for offset, value in enumerate(fields["decision"], 1):
            number, cells = last + offset, [cell.strip() for cell in value.split("|")]
            numbers.append(f"D{number}")
            arch = insert_row(arch, DECISIONS, f"| D{number} | {' | '.join(cells)} |")
            writes[f"docs/solutions/adr/d{number}.md"] = adr.render(number, *cells)
            index = adr.index_lines(index, number, adr.title_of(cells[0])) or index
        cited = [", ".join(numbers)] if numbers else []
        cited += [fields["issue"]] if fields.get("issue") and fields["issue"] not in fields["body"] else []
        prose = re.sub(r"(?<!\\)\|", r"\\|", fields["body"]) + (f" ({'; '.join(cited)})" if cited else "")
        log = insert_row(log, LOG_HEADER, f"| {version} | {date} | {prose} |")
        writes[f"{DIR}/{name}"] = stamp(text, version, numbers)
        plan.append(f"{name} -> {version}, {len(numbers)} decision(s){': ' + ', '.join(numbers) if numbers else ''}")
    return {ARCH: arch, LOG: log, INDEX: index}, writes, plan


def expected_at(root, rev):
    """Every path the recorder would write from `rev`'s tree, with its text; empty when nothing is pending."""
    views, changes = state(root, rev)
    todo = pending(root, rev, changes)
    if not todo:
        return {}
    views, writes, _ = record(views, todo)
    return {**views, **writes}


def main(argv, root=ROOT):
    views, changes = state(root)
    todo = pending(root, "HEAD", changes)
    if not todo:
        print("record: nothing to record")
        return 0
    if len(todo) == len(changes) and "--backfill" not in argv:
        print(f"record: no change file was ever recorded, so these {len(todo)} are a backlog; --backfill records them")
        return 1
    views, writes, plan = record(views, todo)
    print("\n".join(plan))
    if "--dry-run" in argv:
        print(f"record: dry run, {len(todo)} file(s) planned, nothing written")
        return 0
    for path, text in {**views, **writes}.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    print(f"record: {len(todo)} file(s) recorded, version {VERSION.search(views[ARCH]).group(1)}")
    return 0


# The fixture: main at plan time. Twelve real change files whose landing order is not their name order.
PIN = "9ae80aabaa94302c1e499a932cc53084d5a7f69c"
BACKLOG = {  # name -> (version, decisions) as main landed them
    "2026-10-01-change-files.md": ("0.526.0", ["D342"]),
    "2026-10-01-review-scope.md": ("0.527.0", ["D343"]),
    "2026-10-01-family-caps-128.md": ("0.528.0", ["D344"]),
    "2026-10-01-wall-on-the-host.md": ("0.529.0", ["D345", "D346", "D347"]),
    "2026-10-01-host-text-never-user.md": ("0.530.0", ["D348"]),
    "2026-10-01-yi-agent-plan-tool.md": ("0.531.0", []),
    "2026-10-01-acp-close-cancels.md": ("0.532.0", []),
    "2026-10-03-sidecar-with-serve.md": ("0.533.0", ["D349"]),
    "2026-10-03-sandbox-failure-text.md": ("0.534.0", []),
    "2026-10-01-autofix-review-rounds.md": ("0.535.0", ["D350"]),
}
LATER = {"2026-10-03-unlink-through-judged-dir.md": ("0.536.0", ["D351"]),
         "2026-10-03-keys-outside-token-stores.md": ("0.537.0", ["D352"])}


def verify(tmp, recorded, bad):
    """The stamps, the version, and both tables' top rows against `recorded`, newest first."""
    views = {p: (tmp / p).read_text() for p in (ARCH, LOG, INDEX)}
    for name, (version, numbers) in recorded.items():
        fields = parse((tmp / DIR / name).read_text())[0]
        if (fields.get("version"), fields.get("decisions")) != (version, ", ".join(numbers) or None):
            bad.append(f"{name} stamped {fields.get('version')} {fields.get('decisions')}, not {version} {numbers}")
    newest = sorted(recorded.values(), reverse=True)
    rows = [line.split(" | ")[:2] for line in views[LOG].splitlines() if re.match(r"^\| \d+\.\d+\.\d+ \|", line)]
    if [v[2:] for v, _ in rows[:len(newest)]] != [v for v, _ in newest] or VERSION.search(views[ARCH]).group(1) != newest[0][0]:
        bad.append(f"version {VERSION.search(views[ARCH]).group(1)}, changelog rows {rows[:len(newest)]}: not {newest[0][0]} newest first")
    ids = [f"D{m.group(1)}" for row in table(views[ARCH], DECISIONS) if (m := ROW_ID.match(row))]
    want = sorted((d for _, ns in recorded.values() for d in ns), key=lambda d: -int(d[1:]))
    if ids[:len(want)] != want:
        bad.append(f"decision rows run {ids[:len(want)]}, not {want}")
    return views, dict((v[2:], d) for v, d in rows)


def selfcheck():
    if subprocess.run(("git", "-C", str(ROOT), "cat-file", "-e", PIN), capture_output=True).returncode:
        print(f"FAIL record selfcheck\n  the fixture commit {PIN[:9]} is missing; fetch main's full history")
        return 1
    real = landed(ROOT, PIN)
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="yi-record-"))
    bad = []
    run = lambda *a: subprocess.run((sys.executable,) + a, cwd=tmp, capture_output=True, text=True)
    clean = lambda: git(tmp, "status", "--porcelain") == ""

    def sh(*args, date="2026-09-30"):
        at = {"GIT_COMMITTER_DATE": f"{date}T12:00:00", "GIT_AUTHOR_DATE": f"{date}T12:00:00"}
        subprocess.run(("git", "-C", str(tmp), "-c", "user.name=t", "-c", "user.email=t@t") + args,
                       check=True, capture_output=True, env={**os.environ, **at})

    def land(names):
        for name in sorted(names, key=lambda n: real[n][0]):
            (tmp / DIR / name).write_text(git(ROOT, "show", f"{PIN}:{DIR}/{name}"))
            sh("add", f"{DIR}/{name}")
            sh("commit", "-q", "-m", name, date=real[name][1])

    try:
        shutil.copytree(ROOT / "scripts/guardrails", tmp / "scripts/guardrails", ignore=shutil.ignore_patterns("baselines", "__pycache__"))
        for script in ("record_changes.py", "adr.py"):
            shutil.copy(ROOT / "scripts" / script, tmp / "scripts" / script)
        for path in (ARCH, LOG, INDEX):
            (tmp / path).parent.mkdir(parents=True, exist_ok=True)
            (tmp / path).write_text(git(ROOT, "show", f"{PIN}:{path}"))
        (tmp / DIR).mkdir()
        sh("init", "-q", "-b", "main")
        sh("add", "-A")
        sh("commit", "-q", "-m", "seed")
        land(BACKLOG)
        refused = run("scripts/record_changes.py")
        if refused.returncode != 1 or not clean():
            bad.append(f"a backlog without --backfill exited {refused.returncode}, clean={clean()}")
        dry = run("scripts/record_changes.py", "--dry-run", "--backfill")
        if dry.returncode or not clean() or len(dry.stdout.splitlines()) != len(BACKLOG) + 1:
            bad.append(f"--dry-run --backfill exited {dry.returncode}, clean={clean()}: {dry.stdout[-200:]}")
        sh("checkout", "-q", "-b", "topic")
        run("scripts/record_changes.py", "--backfill")
        views, dates = verify(tmp, BACKLOG, bad)
        if dates.get("0.532.0") != real["2026-10-01-acp-close-cancels.md"][1]:
            bad.append(f"0.532.0 is dated {dates.get('0.532.0')}, not the day main landed its file")
        source = parse((tmp / DIR / "2026-10-01-wall-on-the-host.md").read_text())[0]["decision"]
        for number, value in zip((345, 346, 347), source):
            cells = tuple(cell.strip() for cell in value.split("|"))
            if adr.row(number, tmp / ARCH) != cells:
                bad.append(f"D{number} reads back as {adr.row(number, tmp / ARCH)}, not {cells}")
            written = tmp / f"docs/solutions/adr/d{number}.md"
            if not written.is_file() or written.read_text() != adr.render(number, *cells):
                bad.append(f"d{number}.md is not its row rendered")
        if "- [D347](adr/d347.md) - " not in views[INDEX]:
            bad.append("the ADR index lacks D347's line")
        sh("add", "-A")
        accept = run("scripts/guardrails/check_changes.py")
        if accept.returncode:
            bad.append(f"check_changes refused the recorder's own diff: {accept.stdout.strip()}")
        for path, old, new, why in (
            (f"{DIR}/2026-10-01-autofix-review-rounds.md", "version: 0.535.0\n", "", "a version bumped with its file unstamped"),
            (ARCH, "version: 0.535.0", "version: 0.536.0", "a version past the recorder's"),
            (f"{DIR}/2026-10-24-own.md", None, "---\nversion: 0.536.0\n---\nMine.\n", "a branch stamping its own file"),
        ):
            target = tmp / path
            before = target.read_text() if target.exists() else None
            target.write_text(new if before is None else before.replace(old, new, 1))
            if run("scripts/guardrails/check_changes.py").returncode != 1:
                bad.append(f"check_changes accepted {why}")
            if before is None:
                target.unlink()
            else:
                target.write_text(before)
        sh("commit", "-q", "-m", "record")
        if run("scripts/record_changes.py").stdout.strip() != "record: nothing to record" or not clean():
            bad.append("a second run recorded again")
        land(LATER)
        again = run("scripts/record_changes.py")
        if again.returncode:
            bad.append(f"two files landed after the backlog exited {again.returncode}: {again.stdout.strip()}")
        verify(tmp, {**BACKLOG, **LATER}, bad)
    finally:
        shutil.rmtree(tmp)
    if bad:
        print("FAIL record selfcheck\n" + "\n".join(f"  {b}" for b in bad))
        return 1
    print("ok   record selfcheck")
    return 0


if __name__ == "__main__":
    sys.exit(selfcheck() if "--selfcheck" in sys.argv else main(sys.argv[1:]))
