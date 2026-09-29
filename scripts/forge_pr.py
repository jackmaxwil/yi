#!/usr/bin/env python3
"""Finish a branch on the forge with verbs a session cannot get wrong.

The failure modes this replaces were all observed: baselines committed beside code
(commit_style refuses the push), subjects past 72 characters, a `git push` killed
mid-lane by a two-minute timeout, a PR opened against a closed issue (the `title` job
fails and the CLI says nothing), a green PR that will not merge because `main` moved
("head behind base" is only visible through the API), and a merge loop that retried
a refusal it never read. Every verb here does the check first and prints the fix.

Transport is `fgj api`, because fgj already holds the login and trusts the estate's
CA; python's urllib does not. Decisions are pure functions so the selfcheck can walk
them without a server.
"""
import argparse
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "scripts/guardrails"))
from check_pr_metadata import DRAFT  # noqa: E402

BASELINES = ROOT / "scripts/guardrails/baselines"
SUBJECT_LIMIT = 72
POLL = 20
WEB = "https://git.example.invalid"
HOST = WEB.removeprefix("https://")


def git(*args, check=False):
    out = subprocess.run(("git", "-C", str(ROOT)) + args, capture_output=True, text=True, errors="replace", check=False)
    if check and out.returncode != 0:
        raise SystemExit(f"git {' '.join(args)}: {out.stderr.strip()}")
    return out.stdout.strip()


def repo_of(url):
    """`ssh://git@forge:2222/apex/yi.git` and `https://git/apex/yi` both name apex/yi."""
    found = re.search(r"[:/]([^/:]+/[^/]+?)(?:\.git)?/?$", url.strip())
    return found.group(1) if found else None


def repo():
    name = repo_of(git("remote", "get-url", "origin"))
    if not name:
        raise SystemExit("origin is not a forge repository")
    return name


def scope():
    """`-R` and `--hostname` on every fgj verb: a worktree's `.git` is a file fgj cannot read."""
    return ("-R", repo(), "--hostname", HOST)


def fgj_api(method, path, payload=None):
    """A missing resource is None, the way check_pr_metadata's transport expects.

    fgj prints the response body on stdout whatever the status and `HTTP <code>` on
    stderr with exit 1 when it is not 2xx, so a 404 is read from stderr, not parsed."""
    command = ["fgj", "api", "--hostname", HOST, "-X", method, path.lstrip("/")]
    body = None
    if payload is not None:
        command += ["--input", "-"]
        body = json.dumps(payload)
    out = subprocess.run(command, input=body, capture_output=True, text=True, check=False)
    if out.returncode and "HTTP 404" in out.stderr:
        return None
    text = (out.stdout or out.stderr).strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return {"message": text}


def branch():
    return git("rev-parse", "--abbrev-ref", "HEAD")


def pull(number):
    pr = fgj_api("GET", f"repos/{repo()}/pulls/{number}")
    if not pr or "number" not in pr:
        raise SystemExit(f"#{number}: {pr.get('message') if pr else 'no such pull request'}")
    return pr


def pull_for_branch(name):
    pulls = fgj_api("GET", f"repos/{repo()}/pulls?state=open&limit=50") or []
    for pr in pulls:
        if pr.get("head", {}).get("ref") == name:
            return pr
    return None


def pull_number(arg):
    if arg:
        return int(arg)
    pr = pull_for_branch(branch())
    if not pr:
        raise SystemExit(f"no open pull request for {branch()} — just pr open \"<title>\"")
    return pr["number"]


# --- the pure decisions ----------------------------------------------------------


def required_jobs(contexts):
    """`pr / gate (test) (pull_request)` is the job named `gate (test)`."""
    jobs = []
    for context in contexts:
        name = re.sub(r"^\w+ / ", "", context)
        name = re.sub(r" \(pull_request\)$", "", name)
        jobs.append(name)
    return jobs


def job_table(tasks, sha):
    """Newest status per job for one head; a rerun outranks the run it replaced."""
    table = {}
    for task in sorted(tasks, key=lambda task: (task.get("run_number", 0), task.get("updated_at", ""))):
        if task.get("head_sha", "").startswith(sha):
            table[task["name"]] = task["status"]
    return table


def decide(pr, jobs, required, behind):
    if pr.get("merged"):
        return "merged"
    if pr.get("state") != "open":
        return "closed"
    if behind:
        return "behind"
    failed = [job for job in required if jobs.get(job) == "failure"]
    if "title" in failed:
        return "failed:title"
    if failed:
        return f"failed:{failed[0]}"
    # The forge refuses to merge a draft, so polling one for green waits on nothing.
    if pr.get("title", "").startswith(DRAFT):
        return "draft"
    if all(jobs.get(job) == "success" for job in required):
        return "green"
    return "pending"


def ratchet_subject(parts, topic):
    """`Ratchet: test LOC 1 -> 2, tui crate 3 -> 4 for the rail`, cut to the limit
    by dropping the topic, then the parts from the right."""
    parts = list(parts)
    while parts:
        subject = "Ratchet: " + ", ".join(parts) + (f" for {topic}" if topic else "")
        if len(subject) <= SUBJECT_LIMIT:
            return subject
        if topic:
            topic = ""
        else:
            parts.pop()
    return "Ratchet: baselines"


# --- the verbs --------------------------------------------------------------------


def baseline_paths():
    return sorted(str(path.relative_to(ROOT)) for path in BASELINES.iterdir() if path.is_file())


def dirty(paths):
    return [path for path in paths if git("status", "--porcelain", "--", path)]


def read_json(path):
    try:
        return json.loads((ROOT / path).read_text())
    except (OSError, json.JSONDecodeError):
        return {}


def cmd_ratchet(args):
    before = {path: read_json(path) for path in baseline_paths()}
    # Incident: nothing ran check_growth --update, so src_loc.json sat 64 versions back and every
    # memo restated one cumulative number; its update refuses a delta the row has not priced.
    for script in ("check_test_size.py", "check_crate_size.py", "check_comments.py",
                   "check_schemas_lock.py", "check_growth.py", "check_request_budget.py"):
        subprocess.run((sys.executable, str(ROOT / "scripts/guardrails" / script), "--update"), check=False)
    changed = dirty(baseline_paths())
    if not changed:
        print("ratchet: every baseline already matches")
        return binary_ratchet(args.topic) if not args.no_binary else 0
    parts = []
    for path in changed:
        was, now = before.get(path, {}), read_json(path)
        stem = pathlib.Path(path).stem
        if stem == "test_size_budget":
            parts.append(f"test LOC {was.get('budget_lines', '?')} -> {now.get('budget_lines', '?')}")
        elif stem == "binary_size_budget":
            parts.append(f"dist binary {was.get('max_bytes', '?')} -> {now.get('max_bytes', '?')}")
        elif stem == "crate_size_budget":
            for crate, limit in now.items():
                if was.get(crate) != limit:
                    parts.append(f"{crate} crate {was.get(crate, '?')} -> {limit}")
        elif stem == "request_budget":
            for key in ("system", "tools", "total"):
                if was.get(key) != now.get(key):
                    parts.append(f"request budget {key} {was.get(key, '?')} -> {now.get(key, '?')}")
        else:
            parts.append(stem.replace("_", " "))
    subject = ratchet_subject(parts, args.topic)
    git("commit", "-q", "-m", subject, "--", *changed, check=True)
    print(f"ratchet: {subject}")
    return binary_ratchet(args.topic) if not args.no_binary else 0


def cmd_commit(args):
    from check_commit_style import subject_errors

    errs = subject_errors(args.subject.strip())
    if errs:
        for err in errs:
            print(f"subject: {err}")
        return 1
    if dirty(baseline_paths()):
        print("commit: baselines moved — ratcheting them into their own commit first")
        cmd_ratchet(argparse.Namespace(topic=args.topic or "", no_binary=True))
    git("add", "-A", check=True)
    if not git("diff", "--cached", "--name-only"):
        print("commit: nothing to commit")
        return 0
    message = ["-m", args.subject.strip()]
    if args.body:
        message += ["-m", pathlib.Path(args.body).read_text().strip()]
    out = subprocess.run(("git", "-C", str(ROOT), "commit", "-q", *message), check=False)
    if out.returncode != 0:
        return out.returncode
    print(f"committed {git('rev-parse', '--short', 'HEAD')} {args.subject.strip()}")
    return binary_ratchet(args.topic)


def binary_ratchet(topic):
    """Built here, after the code commit: check_binary_size.py only stats target/dist/yi,
    so without a build it measures the previous landing (said ok while the lane grew)."""
    build = subprocess.run(
        (str(ROOT / "scripts/build_dist.sh"),),
        cwd=ROOT, capture_output=True, text=True, check=False,
    )
    if build.returncode != 0:
        print("binary: dist build failed")
        print(build.stderr.strip().splitlines()[-1] if build.stderr.strip() else "")
        return 1
    out = subprocess.run(
        (sys.executable, str(ROOT / "scripts/guardrails/check_binary_size.py")),
        capture_output=True, text=True, check=False,
    )
    text = out.stdout + out.stderr
    verdict = next((line for line in text.splitlines() if "binary_size" in line), "")
    print(f"binary: {verdict.strip() or 'no measurement'}")
    if out.returncode == 0:
        return 0
    # Only a lone budget overrun ratchets; the hard cap, a missing binary or a crash stops the lane.
    failures = [line.strip() for line in text.splitlines() if line.startswith("  ")]
    grown = re.fullmatch(r"dist binary (\d+) > (\d+) bytes", failures[0]) if len(failures) == 1 else None
    if not grown:
        print(text.strip())
        return 1
    path = BASELINES / "binary_size_budget.json"
    path.write_text(f'{{"max_bytes": {grown.group(1)}}}\n')
    subject = ratchet_subject([f"dist binary {grown.group(2)} -> {grown.group(1)}"], topic)
    git("commit", "-q", "-m", subject, "--", str(path.relative_to(ROOT)), check=True)
    print(f"ratchet: {subject}")
    return 0


def cmd_push(args):
    name = branch()
    if git("status", "--porcelain"):
        print("push: the tree is dirty — just commit first")
        return 1
    # The lane refuses a grown binary; measuring here is the last chance before it.
    if binary_ratchet(getattr(args, "topic", "") or ""):
        return 1
    print(f"push: {name}; the forge gate runs the lanes — `just pr status N` reads the verdict")
    out = subprocess.run(("git", "-C", str(ROOT), "push", "-u", "origin", name), check=False)
    return out.returncode


def pushed():
    name = branch()
    remote = git("ls-remote", "origin", f"refs/heads/{name}").split()
    return bool(remote) and remote[0] == git("rev-parse", "HEAD")


def check_problems(title, body):
    import check_pr_metadata as gate

    errs = [f"title: {err}" for err in gate.title_errors(title.strip())]
    errs += gate.template_problems(body)
    measured, why = gate.measure()
    if why:
        return errs + [why]
    ledger_added, changelog_added, src_net, (surface_added, surface_changed) = measured
    errs += gate.surface_problems(body, surface_added, surface_changed)
    errs += gate.body_problems(
        fgj_api, "", repo(), body, [gate.row_key(row) for row in ledger_added], changelog_added, src_net
    )
    return errs


def cmd_check(args):
    if args.number:
        pr = pull(args.number)
        title, body = pr["title"], pr.get("body") or ""
    else:
        title = args.title or ""
        body = pathlib.Path(args.body).read_text() if args.body else ""
    errs = check_problems(title, body)
    for err in errs:
        print(err)
    print("check: " + ("the title job would fail" if errs else "the title job passes"))
    return 1 if errs else 0


def dedupe_counted(body, counted):
    """A section the author already wrote — matched by its level-two heading,
    case-insensitive — wins over the prefill, so `just pr open` never prints a
    section twice. The prefill's own header comment goes too: appended after the
    author's prose it would sit in their last section, which the template check
    then refuses as a placeholder."""
    have = {line[3:].strip().lower() for line in body.splitlines() if line.startswith("## ")}
    out, keep = [], False
    for line in counted.splitlines():
        if line.startswith("## "):
            keep = line[3:].strip().lower() not in have
        if keep:
            out.append(line)
    return "\n".join(out).strip()


def compose_body(args):
    body = pathlib.Path(args.body).read_text().strip() if args.body else ""
    cites = [f"Closes #{n}" for n in args.closes] + [f"Refs #{n}" for n in args.refs]
    if cites:
        body = ", ".join(cites) + ("\n\n" + body if body else "")
    counted = subprocess.run(
        (sys.executable, str(ROOT / "scripts/pr_body.py")), capture_output=True, text=True, check=False
    ).stdout.strip()
    if counted:
        counted = dedupe_counted(body, counted)
    return body + ("\n\n" + counted if counted else "")


def cmd_open(args):
    body = compose_body(args)
    # Every PR opens as a draft; `just pr ready` is what takes the marker off.
    title = DRAFT + args.title.removeprefix(DRAFT)
    errs = check_problems(title, body)
    if errs:
        for err in errs:
            print(err)
        print("open: refused — the title job would fail; fix the above first")
        return 1
    if not pushed():
        code = cmd_push(args)
        if code:
            return code
    out = subprocess.run(
        ("fgj", "pr", "create", *scope(), "--head", branch(), "--base", "main", "--title", title, "--body", body),
        capture_output=True, text=True, check=False,
    )
    found = re.search(r"#(\d+)", out.stdout + out.stderr)
    if not found:
        print((out.stdout + out.stderr).strip())
        return 1
    number = int(found.group(1))
    print(f"#{number} {WEB}/{repo()}/pulls/{number}")
    return 0


def cmd_edit(args):
    payload = {}
    if args.title:
        import check_pr_metadata as gate

        errs = gate.title_errors(args.title)
        if errs:
            for err in errs:
                print(f"title: {err}")
            return 1
        payload["title"] = args.title
    if args.body:
        body = pathlib.Path(args.body).read_text().strip()
        errs = check_problems(payload.get("title", pull(args.number)["title"]), body)
        if errs:
            for err in errs:
                print(err)
            print("edit: refused — the title job would fail; fix the above first")
            return 1
        payload["body"] = body
    if not payload:
        print("edit: nothing to change (--title, --body)")
        return 1
    answer = fgj_api("PATCH", f"repos/{repo()}/pulls/{args.number}", payload)
    if not answer or answer.get("message"):
        print(f"#{args.number} not edited: {(answer or {}).get('message', 'the forge answered 404')}")
        return 1
    print(f"#{args.number} edited")
    return 0


def cmd_ready(args):
    import pr_review

    number = pull_number(args.number)
    pr = pull(number)
    title = pr["title"]
    if not title.startswith(DRAFT):
        print(f"#{number} is not a draft")
        return 0
    rounds = pr_review.rounds_of(pr_review.comments(repo(), number), pr_review.authors())
    errs = pr_review.ready_problems(rounds, pr["head"]["sha"])
    for err in errs:
        print(f"  {err}")
    if errs and pr_review.MODE == "blocking":
        print(f"ready: refused — #{number} stays a draft")
        return 1
    answer = fgj_api("PATCH", f"repos/{repo()}/pulls/{number}", {"title": title.removeprefix(DRAFT)})
    if not answer or answer.get("message"):
        print(f"#{number} not readied: {(answer or {}).get('message', 'the forge answered 404')}")
        return 1
    print(f"#{number} is ready: {title.removeprefix(DRAFT)}")
    return 0


def is_behind(pr):
    """The forge's own refusal reason, computed here so it is seen before the merge."""
    git("fetch", "-q", "origin", "main", pr["head"]["ref"])
    check = subprocess.run(
        ("git", "-C", str(ROOT), "merge-base", "--is-ancestor", "origin/main", pr["head"]["sha"]),
        capture_output=True, check=False,
    )
    return check.returncode != 0


def required():
    protections = fgj_api("GET", f"repos/{repo()}/branch_protections") or []
    for protection in protections:
        if protection.get("branch_name") == "main":
            return required_jobs(protection.get("status_check_contexts") or [])
    return ["gate (guardrails)", "gate (lint)", "gate (test)", "size-report", "title"]


def jobs_of(pr):
    tasks = fgj_api("GET", f"repos/{repo()}/actions/tasks?limit=60") or {}
    return job_table(tasks.get("workflow_runs", []), pr["head"]["sha"])


def report(pr, jobs, need, verdict):
    print(f"#{pr['number']} {pr['title']}")
    print(f"  head {pr['head']['sha'][:8]} ({pr['head']['ref']})  state {pr['state']}"
          + ("  merged" if pr.get("merged") else ""))
    for job in need:
        print(f"  {jobs.get(job, 'not run'):<10} {job}")
    hints = {
        "draft": f"a draft; the forge will not merge it — just pr ready {pr['number']} once it is reviewed",
        "behind": f"main moved under it — just pr update {pr['number']}",
        "failed:title": f"the title job refused — just pr check {pr['number']} prints why",
        "pending": "the gate is still running",
        "green": f"green — just pr merge {pr['number']}",
        "merged": "landed",
        "closed": "closed without merging",
    }
    if verdict.startswith("failed:") and verdict != "failed:title":
        hints[verdict] = (f"{verdict[7:]} failed — logs: `just ci-log {pr['number']}` in the infra repository; "
                          f"push a fix, or `just pr rerun {pr['number']}` if it was the runner")
    print(f"  {hints.get(verdict, verdict)}")


def cmd_status(args):
    pr = pull(pull_number(args.number))
    jobs, need = jobs_of(pr), required()
    verdict = decide(pr, jobs, need, is_behind(pr))
    report(pr, jobs, need, verdict)
    if verdict == "failed:title":
        for err in check_problems(pr["title"], pr.get("body") or ""):
            print(f"  {err}")
    return 0


def cmd_update(args):
    number = pull_number(args.number)
    answer = fgj_api("POST", f"repos/{repo()}/pulls/{number}/update?style=merge")
    if answer and answer.get("message"):
        print(answer["message"])
        return 1
    print(f"#{number} brought up to date with main; the gate reruns")
    return 0


def cmd_rerun(args):
    number = pull_number(args.number)
    subprocess.run(("fgj", "pr", "close", *scope(), str(number)), capture_output=True, check=False)
    time.sleep(2)
    subprocess.run(("fgj", "pr", "reopen", *scope(), str(number)), capture_output=True, check=False)
    print(f"#{number} closed and reopened; the gate reruns")
    return 0


def cmd_merge(args):
    number = pull_number(args.number)
    need = required()
    deadline = time.monotonic() + args.timeout * 60
    while True:
        pr = pull(number)
        jobs = jobs_of(pr)
        verdict = decide(pr, jobs, need, is_behind(pr))
        if verdict == "merged":
            print(f"#{number} merged")
            return 0
        if verdict == "behind":
            print(f"#{number} is behind main; updating")
            cmd_update(argparse.Namespace(number=number))
        elif verdict == "green":
            answer = fgj_api("POST", f"repos/{repo()}/pulls/{number}/merge", {"Do": "merge"})
            if answer and answer.get("message"):
                print(f"merge refused: {answer['message']}")
                if "behind" not in answer["message"]:
                    return 1
        elif verdict.startswith("failed:") or verdict in ("closed", "draft"):
            report(pr, jobs, need, verdict)
            if verdict == "failed:title":
                for err in check_problems(pr["title"], pr.get("body") or ""):
                    print(f"  {err}")
            return 1
        elif not args.wait:
            report(pr, jobs, need, verdict)
            return 2
        if time.monotonic() > deadline:
            print(f"#{number}: gave up after {args.timeout} minutes")
            return 2
        time.sleep(POLL)


def growth_line():
    out = subprocess.run((sys.executable, str(ROOT / "scripts/guardrails/check_growth.py")),
                         capture_output=True, text=True, check=False)
    return out.stdout + out.stderr


def repriced_memo(row, measured):
    """The memo's number follows the measurement; its prose stays the author's."""
    return re.sub(r"growth \+\d+:", f"growth +{measured}:", row, count=1)


def reprice_growth():
    """After a merge the memo trails the tree; `check_growth` says by how much, and only the
    number moves. Returns the commit subject, or None when nothing was owed."""
    text = growth_line()
    if text.strip().startswith("ok"):
        return None
    measured = re.search(r"measurement is ([+-]\d+)", text) or re.search(r"^\s*([+-]\d+) is past", text, re.M)
    if not measured:
        return None
    number = measured.group(1).lstrip("+")
    arch = (ROOT / "docs/ARCHITECTURE.md").read_text()
    version = re.search(r"^version:\s*(\S+)", arch, re.M).group(1)
    log = ROOT / "docs/CHANGELOG.md"
    lines = log.read_text().splitlines()
    for i, line in enumerate(lines):
        if line.startswith(f"| {version} |") and "growth +" in line:
            lines[i] = repriced_memo(line, number)
            log.write_text("\n".join(lines) + "\n")
            subject = f"Price the {version} row at the growth the merge measures"
            git("commit", "-q", "-m", subject, "--", "docs/CHANGELOG.md", check=True)
            return subject
    return None


def render_missing_adrs():
    """A decision row without its ADR is a landing law; `just adr` writes it from the row."""
    arch = (ROOT / "docs/ARCHITECTURE.md").read_text()
    written = []
    for number in re.findall(r"^\| D(\d+) \|", arch, re.M):
        path = ROOT / "docs/solutions/adr" / f"d{number}.md"
        if not path.exists():
            subprocess.run((sys.executable, str(ROOT / "scripts/adr.py"), number), check=True)
            written.append(number)
    if written:
        git("add", "docs/solutions", check=True)
        git("commit", "-q", "-m", "Record " + ", ".join(f"D{n}" for n in written) + " from the decision log", check=True)
    return written


def refresh_from_main():
    """Merge origin/main in, let the baseline driver take the conflicts it owns, re-measure."""
    git("fetch", "--no-tags", "origin", "main", check=True)
    subprocess.run(("git", "-C", str(ROOT), "config", "merge.baseline.driver",
                    f"{sys.executable} scripts/merge_baseline.py %O %A %B"), check=False)
    if subprocess.run(("git", "-C", str(ROOT), "merge-base", "--is-ancestor", "origin/main", "HEAD")).returncode == 0:
        print("land: up to date with origin/main")
        return 0
    merged = subprocess.run(("git", "-C", str(ROOT), "merge", "--no-edit", "origin/main"), capture_output=True, text=True)
    if merged.returncode != 0:
        left = git("diff", "--name-only", "--diff-filter=U")
        print("land: origin/main merged with conflicts a person resolves:")
        for path in left.splitlines():
            print(f"  {path}")
        return 1
    print("land: merged origin/main")
    return 0


def cmd_land(args):
    if refresh_from_main():
        return 1
    # Incident: the merge kept this branch's src_loc.json, a pair that had never seen main's
    # growth, so the gate read main's lines as this version's and no reprice could say so.
    # Main's pair is the last priced point: the row prices everything since, then the
    # ratchet moves the pair past it.
    growth = "scripts/guardrails/baselines/src_loc.json"
    (ROOT / growth).write_text(git("show", f"origin/main:{growth}", check=True) + "\n")
    if subject := reprice_growth():
        print(f"land: {subject}")
    cmd_ratchet(argparse.Namespace(topic=args.title, no_binary=False))
    if written := render_missing_adrs():
        print("land: ADRs rendered for " + ", ".join(f"D{n}" for n in written))
    code = cmd_open(args)
    if code:
        return code
    # A PR lands through its review rounds: until two are clean on its head it stays a draft.
    args.number = None
    if cmd_ready(args):
        print("land: opened as a draft; the review bot reads it — `just pr merge` after `just pr ready` passes")
        return 1
    args.wait = True
    return cmd_merge(args)


def selfcheck():
    assert repo_of("ssh://git@forge.example.invalid:2222/apex/yi.git") == "apex/yi"
    assert repo_of("https://git.example.invalid/apex/yi") == "apex/yi"
    assert repo_of("git@github.com:jackmaxwil/yi.git") == "jackmaxwil/yi"
    assert required_jobs(["pr / gate (test) (pull_request)", "pr / title (pull_request)"]) == [
        "gate (test)", "title",
    ]
    tasks = [
        {"name": "title", "status": "failure", "run_number": 1, "head_sha": "abc123", "updated_at": "1"},
        {"name": "title", "status": "success", "run_number": 2, "head_sha": "abc123", "updated_at": "2"},
        {"name": "gate (test)", "status": "success", "run_number": 2, "head_sha": "abc123", "updated_at": "2"},
        {"name": "gate (test)", "status": "failure", "run_number": 3, "head_sha": "other", "updated_at": "3"},
    ]
    jobs = job_table(tasks, "abc123")
    assert jobs == {"title": "success", "gate (test)": "success"}, "a rerun outranks, another head is ignored"
    need = ["gate (test)", "title"]
    open_pr = {"state": "open", "merged": False}
    assert decide({"merged": True}, {}, need, False) == "merged"
    assert decide(open_pr, jobs, need, True) == "behind", "behind is judged before the jobs"
    assert decide(open_pr, jobs, need, False) == "green"
    assert decide(open_pr, {"title": "success"}, need, False) == "pending"
    assert decide(open_pr, {"title": "failure", "gate (test)": "failure"}, need, False) == "failed:title"
    assert decide(open_pr, {"title": "success", "gate (test)": "failure"}, need, False) == "failed:gate (test)"
    assert decide({"state": "closed", "merged": False}, {}, need, False) == "closed"
    draft = dict(open_pr, title=DRAFT + "Keep the gate")
    assert decide(draft, jobs, need, False) == "draft", "a green draft is still a draft"
    assert decide(draft, {"title": "success", "gate (test)": "failure"}, need, False) == "failed:gate (test)", "a red job outranks the draft"
    assert decide(dict(open_pr, title="Keep the WIP: marker out"), jobs, need, False) == "green"
    short = ratchet_subject(["test LOC 1 -> 2"], "the rail")
    assert short == "Ratchet: test LOC 1 -> 2 for the rail", short
    long = ratchet_subject(["test LOC 36636 -> 36696", "tui crate 10922 -> 10984", "dist binary 5696512 -> 5696544"], "the console pane and its avatars")
    assert len(long) <= SUBJECT_LIMIT and long.startswith("Ratchet: test LOC"), long
    assert ratchet_subject([], "x") == "Ratchet: baselines"
    row = "| 0.150.0 | d | x. growth +1026: measured under D119; prose. |"
    assert repriced_memo(row, "4315") == "| 0.150.0 | d | x. growth +4315: measured under D119; prose. |"
    assert repriced_memo("| 0.1.0 | d | no memo |", "9") == "| 0.1.0 | d | no memo |"
    counted = "<!-- prefilled -->\n\n## Summary\n\nprefill\n\n## Files edited\n\nprefill files\n"
    kept = dedupe_counted("## Summary\n\nmine\n", counted)
    assert "mine" not in kept and "prefill files" in kept and kept.count("## Summary") == 0, kept
    kept = dedupe_counted("## summary\n\nmine\n", counted)
    assert "## Summary" not in kept, "the heading match is case-insensitive"
    assert dedupe_counted("", counted) == counted.split("\n", 2)[2].strip(), "no author prose keeps every section"
    assert "<!--" not in dedupe_counted("## Summary\n\nmine\n", counted), "the header comment never lands in a section"
    assert dedupe_counted("## Summary\n\na\n\n## Files edited\n\nb\n", counted) == "", "nothing owed, nothing appended"
    # Incident: measure() grew the surface delta and this unpack still took three, so
    # `just pr open`, `pr check` and `land` died on a ValueError before asking anything.
    import check_pr_metadata as gate

    real = gate.measure, globals()["repo"]
    gate.measure = lambda: (([], [], 0, ([], ["tool:bash"])), None)
    globals()["repo"] = lambda: "apex/yi"
    filled = "".join(f"## {title}\n\nwords\n\n" for title in gate.REQUIRED)
    try:
        errs = check_problems("Lock the tool surface", filled)
        drafted = check_problems(DRAFT + "Lock the tool surface", "## Summary\n\nwords\n")
    finally:
        gate.measure, globals()["repo"] = real
    assert len(errs) == 1 and "## Claims ledger" in errs[0] and "tool:bash" in errs[0], errs
    assert not [e for e in drafted if e.startswith("title:")], "a draft title passes the title rule"
    assert any("no `## Why needed`" in e for e in drafted), "`just pr open` runs the template check"
    # Incident: the D249 hard-cap line missed the ratchet regex, so `just push` went on.
    import contextlib, io

    capped = "FAIL binary_size\n  dist binary 9000000 > hard cap 8388608 bytes (8 MiB, D249)\n"
    real_run = subprocess.run
    subprocess.run = lambda cmd, **kw: subprocess.CompletedProcess(cmd, int("check_binary" in cmd[-1]), capped, "")
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            stopped = binary_ratchet("")
    finally:
        subprocess.run = real_run
    assert stopped == 1, "a hard-cap breach stops the push lane"
    import pr_review

    real_fgj, real_pull = globals()["fgj_api"], globals()["pull"]
    real_rounds = pr_review.comments, pr_review.authors
    globals()["pull"] = lambda n: {"number": n, "title": DRAFT + "Keep the gate", "head": {"sha": "abc1234"}}
    globals()["fgj_api"] = lambda method, path, payload=None: {"message": "edits are forbidden"}
    clean = [{"id": n, "user": {"login": "jack"}, "body": f"<!-- yi-round {n} -->\n<!-- yi-round-meta pr=7 sha=abc1234 verdict=clean -->\n"} for n in (1, 2)]
    pr_review.authors = lambda: {"jack"}
    try:
        pr_review.comments = lambda repo, number: []
        with contextlib.redirect_stdout(io.StringIO()) as said:
            unread = cmd_ready(argparse.Namespace(number=7))
        pr_review.comments = lambda repo, number: clean
        with contextlib.redirect_stdout(io.StringIO()) as patched:
            stopped = cmd_ready(argparse.Namespace(number=7))
    finally:
        globals()["fgj_api"], globals()["pull"] = real_fgj, real_pull
        pr_review.comments, pr_review.authors = real_rounds
    assert unread == 1 and "refused" in said.getvalue(), "a draft with no rounds is not readied"
    assert stopped == 1 and "not readied" in patched.getvalue(), "a refused title PATCH stops the ready verb"
    assert "0 review round(s)" in said.getvalue(), "ready says what the rounds still owe"
    real_measure = gate.measure
    gate.measure = lambda: (([], [], 0, ([], [])), None)
    body_file = pathlib.Path(tempfile.gettempdir()) / "forge_pr_selfcheck_body.md"
    body_file.write_text("## Summary\n\nwords\n")
    globals()["pull"] = lambda n: {"number": n, "title": "Keep the gate"}
    try:
        with contextlib.redirect_stdout(io.StringIO()) as seen:
            stopped = cmd_edit(argparse.Namespace(number=7, title=None, body=str(body_file)))
    finally:
        gate.measure = real_measure
        globals()["pull"] = real_pull
        body_file.unlink()
    assert stopped == 1, "an incomplete body is refused by edit"
    globals()["fgj_api"] = lambda method, path, payload=None: {"message": "pull request is closed"}
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            stopped = cmd_edit(argparse.Namespace(number=7, title="Keep the gate", body=None))
    finally:
        globals()["fgj_api"] = real_fgj
    assert stopped == 1, "a refused PATCH is not reported as an edit"
    assert any("no `## Why needed`" in line for line in seen.getvalue().splitlines()), seen.getvalue()
    # Incident: review round 1's fixer deleted `def cmd_commit` and every hook stayed green,
    # because nothing here built the verbs; a name the parser wires that no longer exists
    # now fails this flag instead of the next `just commit`.
    build_parser()
    print("ok   forge_pr selfcheck")


def build_parser():
    parser = argparse.ArgumentParser(prog="just")
    verbs = parser.add_subparsers(dest="verb", required=True)
    ratchet = verbs.add_parser("ratchet")
    ratchet.add_argument("topic", nargs="?", default="")
    ratchet.add_argument("--no-binary", action="store_true", help="skip the dist build")
    ratchet.set_defaults(run=cmd_ratchet)
    commit = verbs.add_parser("commit")
    commit.add_argument("subject")
    commit.add_argument("--body")
    commit.add_argument("--topic", default="")
    commit.set_defaults(run=cmd_commit)
    verbs.add_parser("push").set_defaults(run=cmd_push)
    pr = verbs.add_parser("pr").add_subparsers(dest="pr_verb", required=True)

    def opener(name, run):
        sub = pr.add_parser(name) if name != "land" else verbs.add_parser(name)
        sub.add_argument("title")
        sub.add_argument("--body")
        sub.add_argument("--closes", action="append", default=[])
        sub.add_argument("--refs", action="append", default=[])
        sub.add_argument("--timeout", type=int, default=40)
        sub.set_defaults(run=run)

    opener("open", cmd_open)
    opener("land", cmd_land)
    check = pr.add_parser("check")
    check.add_argument("number", nargs="?")
    check.add_argument("--title")
    check.add_argument("--body")
    check.set_defaults(run=cmd_check)
    edit = pr.add_parser("edit")
    edit.add_argument("number", type=int)
    edit.add_argument("--title")
    edit.add_argument("--body")
    edit.set_defaults(run=cmd_edit)
    import pr_review

    for name, run in (("status", cmd_status), ("ready", cmd_ready), ("update", cmd_update), ("rerun", cmd_rerun),
                      ("review", pr_review.cmd_review), ("fix", pr_review.cmd_fix)):
        sub = pr.add_parser(name)
        sub.add_argument("number", nargs="?")
        sub.set_defaults(run=run)
    pr.choices["review"].add_argument("--dry-run", action="store_true", help="print the round, post nothing")
    pr.choices["review"].add_argument("--again", action="store_true", help="read a head the rule says is read enough")
    pr.add_parser("sweep").set_defaults(run=pr_review.cmd_sweep)
    replay = pr.add_parser("replay")
    replay.add_argument("numbers", nargs="+", type=int)
    replay.add_argument("--label", required=True, help="what the PR is known to be: bad, kept, closed")
    replay.add_argument("--out", required=True, help="the JSON-lines file each reading is appended to")
    replay.set_defaults(run=pr_review.cmd_replay)
    merge = pr.add_parser("merge")
    merge.add_argument("number", nargs="?")
    merge.add_argument("--no-wait", dest="wait", action="store_false")
    merge.add_argument("--timeout", type=int, default=40)
    merge.set_defaults(run=cmd_merge)
    return parser


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
