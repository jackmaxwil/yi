#!/usr/bin/env python3
"""Three rules a workflow file is refused for, all about the forge reading the file rather than YAML:
a plain value holds no `: ` (the mapping incident below), a commit message a workflow pushes carries
no skip keyword, which starts no workflow at all for that push, and a review job never runs on a
gate job's runner lane.
Incident: `if: ... startsWith(title, 'WIP: ')` failed three review runs before any job existed
("mapping values are not allowed in this context"); the stdlib has no YAML parser."""
import re, shutil, subprocess, sys, pathlib, tempfile
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

KEY = re.compile(r"^\s*(?:-\s+)?[\w.$-]+:(?:\s+(.*))?$")
BLOCK = re.compile(r"^[|>][-+0-9]*\s*(#.*)?$")
NOT_PLAIN = tuple("'\"|>{[&*!#")
SKIP = re.compile(r"\[\s*(?:skip[ _-]?ci|ci[ _-]?skip|no[ _-]?ci|skip[ _-]?actions|actions[ _-]?skip)\s*\]", re.I)

JOBS = re.compile(r"^  ([\w-]+):\s*$")
RUNNER = re.compile(r"^    runs-on:(?:\s+(.*?))?\s*$")
ITEM = re.compile(r"^\s+-\s+(.*?)\s*$")


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


def label(value):
    """Lanes named by one `runs-on` value: bare, quoted, or a `[a, b]` flow list; none for an expression."""
    parts = value[1:-1].split(",") if value.startswith("[") and value.endswith("]") else [value]
    out = []
    for part in parts:
        part = part.strip()
        if len(part) > 1 and part[0] == part[-1] and part[0] in "'\"":
            part = part[1:-1]
        if not part or part.startswith("${{") or part.endswith("}}"):
            return []
        out.append(part)
    return out


def lanes(text):
    """({job: [lanes]}, [(job, value)]) from a workflow's `jobs:`; a `runs-on` this cannot read is
    reported, never dropped: a lane nobody compared is the revert this rule exists to refuse."""
    out, unread, job, items = {}, [], None, None
    for line in text.splitlines():
        if items is not None:
            m = ITEM.match(line)
            if m and len(line) - len(line.lstrip()) > 4:
                out[items].extend(label(m.group(1)))
                continue
            if not out.get(items):
                unread.append((items, ""))
            items = None
        if line.startswith("jobs:"):
            job = None
        elif JOBS.match(line):
            job = JOBS.match(line).group(1)
        elif job and RUNNER.match(line):
            value = (RUNNER.match(line).group(1) or "").split(" #")[0].strip()
            names = label(value)
            if names:
                out.setdefault(job, []).extend(names)
            elif value:
                unread.append((job, value))
            else:
                out.setdefault(job, [])
                items = job
    if items is not None and not out.get(items):
        unread.append((items, ""))
    return out, unread


def shared_lane(gate, review, off=""):
    """(job, lane) of every review job on a lane a pr.yml gate job of the same estate runs on."""
    if off == "lane":
        return []
    gate_lanes = {lane for names in lanes(gate)[0].values() for lane in names}
    return sorted((job, lane) for job, names in lanes(review)[0].items() for lane in names if lane in gate_lanes)


def unread_lane(text, off=""):
    """`runs-on` values the lane rule could not read; an expression there is a lane nobody compared."""
    return [] if off == "lane" else lanes(text)[1]


def no_skills(text, off=""):
    return off != "skills" and not re.search(r"^\s*(- )?run: just install-skills$", text, re.M)


def shipped():
    """Where the install puts each skill: `.ruler/skills` at the root, the bot's own under `yi/`."""
    return sorted([p.name for p in (ROOT / ".ruler/skills").iterdir() if (p / "SKILL.md").is_file()]
                  + [f"yi/{p.name}" for p in (ROOT / "skills/yi").iterdir() if (p / "SKILL.md").is_file()])


def skill_gaps(dest, off=""):
    """Skills the install left out of `dest`, at the depth a prompt opens them: `dest/<name>/SKILL.md`."""
    if off == "install":
        return []
    return [name for name in shipped() if not (dest / name / "SKILL.md").is_file()]


def selfcheck(off=""):
    incident = "jobs:\n  review:\n    if: github.head == github.repo && startsWith(github.title, 'WIP: ')\n"
    fixed = "jobs:\n  review:\n    if: >-\n      startsWith(github.title, 'WIP: ')\n"
    fine = "steps:\n  - name: 'quoted: fine'\n  - run: |\n      echo a: b\n      note: a: b\n  - run: echo done # note: ok\n"
    quiet = 'record:\n  - run: |\n      git add docs\n      git commit -q -m "record" -m "[skip ci]"\n'
    gate = "jobs:\n  gate:\n    runs-on: ubuntu-latest\n"
    shared = "jobs:\n  review:\n    runs-on: ubuntu-latest\n"
    apart = "jobs:\n  review:\n    runs-on: llm\n"
    noted = "jobs:\n  review:\n    runs-on: ubuntu-latest  # back on the gate lane\n"
    quoted = "jobs:\n  review:\n    runs-on: \"ubuntu-latest\"\n"
    flow = "jobs:\n  review:\n    runs-on: [self-hosted, ubuntu-latest]\n"
    listed = "jobs:\n  review:\n    runs-on:\n      - self-hosted\n      - ubuntu-latest\n"
    expr = "jobs:\n  review:\n    runs-on: ${{ matrix.os }}\n"
    bad = []
    if [n for n, _ in problems(incident, off)] != [3]:
        bad.append("the incident's plain `if:` holding 'WIP: ' is not refused")
    if problems(fixed, off) or problems(fine, off):
        bad.append("a folded value, a quoted one or a `run: |` block's text or a trailing comment is refused")
    if [n for n, _ in silenced(quiet, off)] != [4]:
        bad.append("a commit message carrying [skip ci] is not refused")
    if silenced('# a comment naming [skip ci] is not a commit\nrun: echo one\n', off):
        bad.append("a comment naming the keyword is refused")
    if shared_lane(gate, shared, off) != [("review", "ubuntu-latest")]:
        bad.append("a review job on the gate's own runner lane is not refused")
    spellings = (("a trailing comment", noted), ("a quoted label", quoted), ("a flow list", flow), ("a listed lane", listed))
    for name, text in spellings:
        if shared_lane(gate, text, off) != [("review", "ubuntu-latest")]:
            bad.append(f"a review `runs-on` spelled as {name} is not read as the gate's lane")
    if shared_lane(gate, apart, off):
        bad.append("a review job on a lane of its own is refused")
    if not unread_lane(expr, off):
        bad.append("a `runs-on` that is an expression is dropped instead of refused")
    if not no_skills("steps:\n  - run: rustup show\n", off):
        bad.append("a bot job that installs no skills is not refused")
    if no_skills("steps:\n  - run: just install-skills\n", "") or no_skills("steps:\n  - if: x\n    run: just install-skills\n", ""):
        bad.append("a bot job that installs the skills is refused")
    short = pathlib.Path(tempfile.mkdtemp(prefix="yi-workflows-skills-"))
    try:
        if not skill_gaps(short, off):
            bad.append("an install that lands no skill at all is refused by nothing")
        for name in shipped():
            (short / name).mkdir(parents=True)
            (short / name / "SKILL.md").write_text("---\n")
        if skill_gaps(short, off):
            bad.append("an install that lands every skill the repository ships is refused")
        (short / "har/SKILL.md").unlink()
        if not skill_gaps(short, off):
            bad.append("an install whose skill lands one level off is refused by nothing")
    finally:
        shutil.rmtree(short, ignore_errors=True)
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("plain", "blocks", "comments", "silenced", "lane", "skills", "install"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        fail(errs, "workflows selfcheck")
        sys.exit(0)
    files = sorted((ROOT / ".forgejo/workflows").glob("*.y*ml"))
    gate = (ROOT / ".forgejo/workflows/pr.yml").read_text()
    review = (ROOT / ".forgejo/workflows/review.yml").read_text()
    errs = [f"{f.relative_to(ROOT)}:{n}: `{v}` is a plain YAML value holding `: `, which starts a mapping; "
            f"quote it or fold it with `>-`" for f in files for n, v in problems(f.read_text())]
    errs += [f"{f.relative_to(ROOT)}:{n}: `{v}` starts no workflow for the push that carries it, the mirror's "
             f"run included; skip the lane that must not re-run instead, by author or by path"
             for f in files for n, v in silenced(f.read_text())]
    errs += [f".forgejo/workflows/review.yml: job {job} runs on {lane}, the lane a pr.yml gate job runs on; "
             "a round waits an hour or two on the model at a few percent of one CPU, so the gates queued behind "
             "it for up to four hours (#1310): give the round a lane of its own"
             for job, lane in shared_lane(gate, review)]
    errs += [f".forgejo/workflows/{name}: job {job}'s `runs-on` `{value}` is not a lane this check reads; "
             "an expression there is a review lane nobody compared, so name the label"
             for name, text in (("pr.yml", gate), ("review.yml", review)) for job, value in unread_lane(text)]
    errs += [f".forgejo/workflows/{name}: no `run: just install-skills` step; the job's HOME then holds no skills and its "
             "footer says `skills read none`"
             for name in ("review.yml", "autofix.yml") if no_skills((ROOT / ".forgejo/workflows" / name).read_text())]
    dest = pathlib.Path(tempfile.mkdtemp(prefix="yi-install-skills-"))
    try:
        ran = subprocess.run(["just", "install-skills", str(dest)], cwd=ROOT, capture_output=True, text=True)
        gaps = skill_gaps(dest)
        errs += ([f"`just install-skills` exited {ran.returncode}: {(ran.stdout + ran.stderr).strip()[-400:]}"]
                 if ran.returncode else
                 [f"`just install-skills <dest>` landed no {name}/SKILL.md; a bot's prompt then reads no skills "
                  f"and its footer says `skills read none`" for name in gaps])
    except OSError as err:
        errs.append(f"`just install-skills <dest>` did not run: {err}")
    finally:
        shutil.rmtree(dest, ignore_errors=True)
    fail(errs, "workflows")
