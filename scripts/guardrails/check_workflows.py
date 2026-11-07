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
CHOSEN = re.compile(r"\$\{\{ contains\([^{}()&|]*\) && '([\w-]+)' \|\| '([\w-]+)' \}\}")


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
    if chosen := CHOSEN.fullmatch(value):
        return list(chosen.groups())
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


def block(text, head):
    """The lines under `head` (an exact line) up to the next key indented no deeper; empty if absent."""
    lines = text.splitlines()
    if head not in lines:
        return []
    start, out = lines.index(head) + 1, []
    depth = len(head) - len(head.lstrip())
    for line in lines[start:]:
        if line.strip() and not line.lstrip().startswith("#") and len(line) - len(line.lstrip()) <= depth:
            break
        out.append(line)
    while out and (not out[-1].strip() or out[-1].lstrip().startswith("#")):
        out.pop()
    return out


def gate_part(text, head):
    jobs = text.splitlines()
    return block("\n".join(jobs[jobs.index("  gate:"):]), head) if "  gate:" in jobs else []


def same_gate(gate, candidate, off=""):
    """Which of env and the gate's lanes and steps candidate.yml does not share with pr.yml: the queue lands
    what its gate passed, so a lane it runs differently is a check main never had."""
    if off == "candidate":
        return []
    parts = (("env", lambda t: block(t, "env:")), ("gate lanes", lambda t: gate_part(t, "    strategy:")),
             ("gate steps", lambda t: gate_part(t, "    steps:")))
    return [name for name, part in parts if not part(candidate) or part(candidate) != part(gate)]


def off_queue(candidate, off=""):
    """Whether candidate.yml's gate asks for anything but the `queue` runner, the one change it exists for."""
    return off != "candidate" and lanes(candidate)[0].get("gate") != ["queue"]


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


RECORDER_CHECKS = (
    "record-timeout", "record-checkout", "record-toolchain", "record-ratchet", "record-bundle",
    "record-upload", "record-output", "push-dependency", "push-download", "push-token", "push-clean", "push-result",
)


def workflow_job(text, name):
    jobs = text.rsplit("\njobs:\n", 1)[-1]
    return "\n".join(block(jobs, f"  {name}:"))


def workflow_steps(job):
    lines = block(job, "    steps:")
    starts = [n for n, line in enumerate(lines) if line.startswith("      - ")]
    return ["\n".join(lines[start:end]) for start, end in zip(starts, [*starts[1:], len(lines)])]


def workflow_step(steps, marker):
    return next((step for step in steps if marker in step), "")


def recorder_isolation(text, off=""):
    """The recorder builds untrusted landed code; only its artifact-fed push job may hold the main token."""
    record, push = workflow_job(text, "record"), workflow_job(text, "push")
    steps, push_steps = workflow_steps(record), workflow_steps(push)
    checkout = workflow_step(steps, "uses: actions/checkout@")
    ratchet = workflow_step(steps, "run: python3 scripts/forge_pr.py ratchet --forge-measured")
    bundle = workflow_step(steps, 'run: git bundle create "$RUNNER_TEMP/recorder.bundle" HEAD')
    upload = workflow_step(steps, "uses: actions/upload-artifact@")
    download = workflow_step(push_steps, "uses: actions/download-artifact@")
    errs = []

    def want(name, condition, message):
        if off != name and not condition:
            errs.append(message)

    want("record-timeout", "    timeout-minutes: 20" in record,
         "the recorder's cold dist build needs the PR lane's 20-minute budget")
    want("record-checkout", checkout and "persist-credentials: false" in checkout
         and "token: ${{ secrets.RECORDER_TOKEN }}" in checkout and record.count("RECORDER_TOKEN") == 1,
         "the recorder checkout alone may read its token and must not persist it")
    want("record-toolchain", workflow_step(steps, "run: rustup show")
         and workflow_step(steps, "uses: Swatinem/rust-cache@"), "the dist build needs its toolchain and cache")
    want("record-ratchet", ratchet and "id: ratchet" in ratchet and "continue-on-error: true" in ratchet
         and all(name in ratchet for name in ("GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL")),
         "the ratchet step needs an id, continue-on-error and all four Git identity values")
    want("record-bundle", bundle and "if: always() && steps.ratchet.outcome != 'skipped'" in bundle,
         "the recorder must bundle HEAD unless an earlier failure skipped the ratchet")
    want("record-upload", upload and "name: recorder" in upload and "path: ${{ runner.temp }}/recorder.bundle" in upload,
         "the recorder must upload its bundle as the recorder artifact")
    want("record-output", "ratchet_outcome: ${{ steps.ratchet.outcome }}" in record,
         "the record job must export the ratchet outcome to the push job")
    want("push-dependency", "    needs: [record]" in push
         and "if: vars.RECORD_CHANGES == 'on' && github.event.head_commit.author.name != 'yi-queue'" in push,
         "the push job must depend on record and use its enable condition")
    want("push-download", download and "name: recorder" in download and "path: ${{ runner.temp }}" in download
         and not workflow_step(push_steps, "uses: actions/checkout@"),
         "the push job must download the recorder artifact without checking out landed code")
    want("push-token", "RECORDER_TOKEN: ${{ secrets.RECORDER_TOKEN }}" in push
         and "RATCHET_OUTCOME: ${{ needs.record.outputs.ratchet_outcome }}" in push,
         "only the push job may receive the token and it must read the record job's ratchet outcome")
    want("push-clean", 'mktemp -d "$RUNNER_TEMP/recorder-push' in push
         and "clone --template= --branch main --single-branch" in push and 'git -C "$push_workspace"' in push
         and "GIT_CONFIG_GLOBAL=/dev/null" in push and "GIT_CONFIG_NOSYSTEM=1" in push
         and push.count("core.hooksPath=/dev/null") == 3 and push.count("core.fsmonitor=false") == 3
         and "http.extraheader" in push and 'fetch "$RUNNER_TEMP/recorder.bundle" HEAD:refs/heads/recording' in push,
         "the token-bearing clone, fetch and push must use a fresh clone with inherited Git behavior disabled")
    want("push-result", 'test "$RATCHET_OUTCOME" = "success"' in push,
         "a pushed ratchet failure must still fail the push job")
    return errs


def mutate_job(text, name, old, new):
    job = workflow_job(text, name)
    changed = job.replace(old, new, 1)
    return text.replace(job, changed, 1) if changed != job else text


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
    chosen = "jobs:\n  review:\n    runs-on: ${{ contains(github.event.pull_request.labels.*.name, 'x') && 'x' || 'ubuntu-latest' }}\n"
    if shared_lane(gate, chosen, off) != [("review", "ubuntu-latest")]:
        bad.append("a `runs-on` choosing between two named lanes is not read as both")
    two = ("env:\n  A: 1\njobs:\n  gate:\n    strategy:\n      matrix:\n        lane: [a, b]\n    runs-on: a\n"
           "    steps:\n# a note\n      - run: one\n      - run: three\n\n  # the next job\n  title:\n    steps:\n      - run: x\n")
    if same_gate(two, two.replace("runs-on: a", "runs-on: queue").replace("  title:\n    steps:\n      - run: x\n", ""), off):
        bad.append("a candidate gate whose env and steps are pr.yml's is refused")
    if any(not same_gate(two, two.replace(a, b), off) for a, b in (("run: three", "run: two"), ("A: 1", "A: 2"), ("[a, b]", "[a]"))):
        bad.append("a candidate gate whose env, lanes or a step past a comment differ from pr.yml's is not refused")
    if not off_queue(two, off) or off_queue(two.replace("runs-on: a", "runs-on: queue"), off):
        bad.append("a candidate gate on any runner but `queue` is not refused, or the `queue` one is")
    if not unread_lane("jobs:\n  review:\n    runs-on: ${{ contains(a) || contains(b) && 'x' || 'y' }}\n", off):
        bad.append("a `runs-on` that can yield a condition rather than a lane is read as lanes")
    if not unread_lane(expr, off):
        bad.append("a `runs-on` that is an expression is dropped instead of refused")
    if not no_skills("steps:\n  - run: rustup show\n", off):
        bad.append("a bot job that installs no skills is not refused")
    if no_skills("steps:\n  - run: just install-skills\n", "") or no_skills("steps:\n  - if: x\n    run: just install-skills\n", ""):
        bad.append("a bot job that installs the skills is refused")
    postmerge = (ROOT / ".forgejo/workflows/postmerge.yml").read_text()
    bad.extend(f"the current postmerge workflow: {err}" for err in recorder_isolation(postmerge, off))
    mutations = {
        "record-timeout": ("record", "    timeout-minutes: 20", "    timeout-minutes: 10"),
        "record-checkout": ("record", "persist-credentials: false", "persist-credentials: true"),
        "record-toolchain": ("record", "- run: rustup show", "- run: rustup --version"),
        "record-ratchet": ("record", "continue-on-error: true", "continue-on-error: false"),
        "record-bundle": ("record", 'git bundle create "$RUNNER_TEMP/recorder.bundle" HEAD', 'git bundle create "$RUNNER_TEMP/recorder.bundle" main'),
        "record-upload": ("record", "name: recorder", "name: wrong-artifact"),
        "record-output": ("record", "steps.ratchet.outcome", "steps.other.outcome"),
        "push-dependency": ("push", "    needs: [record]\n", ""),
        "push-download": ("push", "name: recorder", "name: wrong-artifact"),
        "push-token": ("push", "needs.record.outputs.ratchet_outcome", "steps.ratchet.outcome"),
        "push-clean": ("push", "clone --template=", "clone"),
        "push-result": ("push", 'test "$RATCHET_OUTCOME" = "success"', 'test "$RATCHET_OUTCOME" = "failure"'),
    }
    for name, (job, old, new) in mutations.items():
        changed = mutate_job(postmerge, job, old, new)
        if changed == postmerge:
            bad.append(f"the recorder selfcheck could not make its {name} mutation")
        elif not recorder_isolation(changed, off):
            bad.append(f"the {name} recorder rule does not refute its mutation")
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
        for off in ("plain", "blocks", "comments", "silenced", "lane", "candidate", "skills", "install", *RECORDER_CHECKS):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        fail(errs, "workflows selfcheck")
        sys.exit(0)
    files = sorted((ROOT / ".forgejo/workflows").glob("*.y*ml"))
    gate = (ROOT / ".forgejo/workflows/pr.yml").read_text()
    review = (ROOT / ".forgejo/workflows/review.yml").read_text()
    postmerge = (ROOT / ".forgejo/workflows/postmerge.yml").read_text()
    candidate_path = ROOT / ".forgejo/workflows/candidate.yml"
    candidate = candidate_path.read_text() if candidate_path.is_file() else ""
    errs = [f"{f.relative_to(ROOT)}:{n}: `{v}` is a plain YAML value holding `: `, which starts a mapping; "
            f"quote it or fold it with `>-`" for f in files for n, v in problems(f.read_text())]
    errs += [f"{f.relative_to(ROOT)}:{n}: `{v}` starts no workflow for the push that carries it, the mirror's "
             f"run included; skip the lane that must not re-run instead, by author or by path"
             for f in files for n, v in silenced(f.read_text())]
    errs += [f".forgejo/workflows/review.yml: job {job} runs on {lane}, the lane a pr.yml gate job runs on; "
             "a round waits an hour or two on the model at a few percent of one CPU, so the gates queued behind "
             "it for up to four hours (#1310): give the round a lane of its own"
             for job, lane in shared_lane(gate + candidate, review)]
    errs += [f".forgejo/workflows/candidate.yml: {part}: not pr.yml's; the queue lands what this gate passed, "
             "so copy pr.yml's" for part in same_gate(gate, candidate)] if candidate else [
        ".forgejo/workflows/candidate.yml is missing: the queue candidate's gate then runs nowhere"]
    errs += [".forgejo/workflows/candidate.yml: the gate's `runs-on` is not `queue`, so the candidate waits behind "
             "PR runs again (#2042)"] if candidate and off_queue(candidate) else []
    errs += [f".forgejo/workflows/{name}: job {job}'s `runs-on` `{value}` is not a lane this check reads; "
             "an expression there is a review lane nobody compared, so name the label"
             for name, text in (("pr.yml", gate), ("candidate.yml", candidate), ("review.yml", review))
             for job, value in unread_lane(text)]
    errs += [f".forgejo/workflows/{name}: no `run: just install-skills` step; the job's HOME then holds no skills and its "
             "footer says `skills read none`"
             for name in ("review.yml", "autofix.yml") if no_skills((ROOT / ".forgejo/workflows" / name).read_text())]
    errs += [f".forgejo/workflows/postmerge.yml: {err}" for err in recorder_isolation(postmerge)]
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
