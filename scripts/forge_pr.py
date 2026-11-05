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
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]


def job_secs(workflow):
    """A bot job's time limit, from the one place it is set: its workflow's `timeout-minutes`.
    The runner's own cap (the infra repository `CI_RUNNER_JOB_TIMEOUT`, 120m) must not be below it."""
    text = (ROOT / ".forgejo/workflows" / f"{workflow}.yml").read_text()
    found = re.search(r"^\s+timeout-minutes:\s*(\d+)\s*$", text, re.M)
    if not found:
        raise ValueError(f"{workflow}.yml sets no timeout-minutes, the job limit the bot budgets from")
    return int(found.group(1)) * 60
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "scripts/guardrails"))
import forge_tracking  # noqa: E402
import merge_baseline  # noqa: E402
from check_pr_metadata import DRAFT  # noqa: E402

BASELINES = ROOT / "scripts/guardrails/baselines"
BINARY = {"binary_size_budget": "dist binary", "binary_size_budget_linux": "Linux dist binary"}
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


def open_pulls():
    """Every open pull request, page by page: the forge returns 50 a page, and a reader of page one
    alone never sees the oldest. Incident: 102 were open and a pass saw #1830..#1891 only."""
    bad = []

    def transport(method, path, body):
        batch = fgj_api(method, path, body)
        if not isinstance(batch, list) and not bad:
            bad.append(batch)
        return batch

    found = list(forge_tracking.paged(transport, f"repos/{repo()}/pulls?state=open"))
    # Incident: an unsigned fgj answered the listing with an error object the loop then indexed, so
    # every page's reply is judged, and a 404 (None) is a failed page, not an empty one.
    return bad[0] if bad else found


def pull_for_branch(name):
    for pr in open_pulls() or []:
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
        name = re.sub(r"^[\w-]+ / ", "", context)
        name = re.sub(r" \((?:pull_request(?:_target)?|push)\)$", "", name)
        jobs.append(name)
    return jobs


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
    # The size ceilings are measured at the fork and raised in a change file; these still store one.
    for script in ("check_schemas_lock.py", "check_request_budget.py"):
        subprocess.run((sys.executable, str(ROOT / "scripts/guardrails" / script), "--update"), check=False)
    changed = dirty(baseline_paths())
    if not changed:
        print("ratchet: every baseline already matches")
        return binary_ratchet(args.topic) if not args.no_binary else 0
    parts = []
    for path in changed:
        was, now = before.get(path, {}), read_json(path)
        stem = pathlib.Path(path).stem
        if stem in BINARY:
            parts.append(binary_part(stem, was.get("max_bytes", "?"), now.get("max_bytes", "?")))
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
    if args.paths:
        git("add", "--", *args.paths, check=True)
    if not git("diff", "--cached", "--name-only"):
        left = git("status", "--porcelain")
        print("commit: nothing staged; name the paths to commit:" if left else "commit: nothing to commit")
        print(left)
        return 1 if left else 0
    message = ["-m", args.subject.strip()]
    if args.body:
        message += ["-m", pathlib.Path(args.body).read_text().strip()]
    out = subprocess.run(("git", "-C", str(ROOT), "commit", "-q", *message, *(("--", *args.paths) if args.paths else ())), check=False)
    if out.returncode != 0:
        return out.returncode
    print(f"committed {git('rev-parse', '--short', 'HEAD')} {args.subject.strip()}")
    return binary_ratchet(args.topic)


def overrun(text):
    """(baseline stem, budget, size) of a lone budget overrun; the hard cap, a missing binary or a
    crash is None and stops the lane."""
    failures = [line.strip() for line in text.splitlines() if line.startswith("  ")]
    grown = re.fullmatch(r"dist binary (\d+) > (\d+) \+ \d+ bytes \((\w+)\.json\)", failures[0]) if len(failures) == 1 else None
    return (grown.group(3), int(grown.group(2)), int(grown.group(1))) if grown and grown.group(3) in BINARY else None


def binary_part(stem, was, now):
    """A size ceiling's half of a `Ratchet:` subject."""
    return f"{BINARY[stem]} {was} -> {now}"


def write_ceiling(path, size):
    path.write_text(merge_baseline.layout({"max_bytes": size}))


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
    grown = overrun(text)
    if not grown:
        print(text.strip())
        return 1
    stem, was, size = grown
    path = BASELINES / f"{stem}.json"
    write_ceiling(path, size)
    subject = ratchet_subject([binary_part(stem, was, size)], topic)
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
        body = compose_body(args)
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
    # A host-run recipe never builds: pushing here would run a child's build.rs outside the sandbox.
    if not pushed():
        print("open: this branch is not pushed — just push first")
        return 1
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
    rounds = pr_review.rounds_of(pr_review.comments(repo(), number), pr_review.authors(), number)
    errs = pr_review.ready_problems(rounds, pr["head"]["sha"], pr_review.holds_for(number, pr["base"]["ref"]))
    for err in errs:
        print(f"  {err}")
    if errs and pr_review.MODE == "blocking":
        print(f"ready: refused — #{number} stays a draft until two review rounds are clean on its head")
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
    """Invariant: a merge waits on the three gates whatever the protection rule lists, because the
    rule is a setting someone can shorten and the merge verb is where red must stop."""
    import pr_review

    protections = fgj_api("GET", f"repos/{repo()}/branch_protections")
    protected = ["title"]
    for protection in protections if isinstance(protections, list) else []:
        if protection.get("branch_name") == "main":
            protected = required_jobs(protection.get("status_check_contexts") or [])
    # Rounds are the review's check (unreviewed): an /override leaves the job's status red for good.
    return [job for job in dict.fromkeys([*pr_review.GATE_LANES, *protected]) if job != "review"]


# Invariant: a 403 or 500 answers with a message and no statuses, which must not read as nothing waiting.
UNREAD = "the commit's statuses, unreadable"


def head_jobs(name, sha):
    """Job name to its newest status on one head. Incident: the task list has no row until a runner
    claims a job, so #1887's waiting rerun read as its old failure, and 60 rows is an hour or two."""
    combined = fgj_api("GET", f"repos/{name}/commits/{sha}/status") or {}
    if "message" in combined:
        return {UNREAD: "pending"}
    jobs = {}
    for status in combined.get("statuses") or []:
        job = required_jobs([status["context"]])[0]
        jobs[job] = "failure" if status["status"] == "error" else status["status"]
    return jobs


def jobs_of(pr):
    return head_jobs(repo(), pr["head"]["sha"])


def report(pr, jobs, need, verdict):
    print(f"#{pr['number']} {pr['title']}")
    print(f"  head {pr['head']['sha'][:8]} ({pr['head']['ref']})  state {pr['state']}"
          + ("  merged" if pr.get("merged") else ""))
    for job in need:
        print(f"  {jobs.get(job, 'not run'):<10} {job}")
    hints = {
        "draft": f"a draft; the forge will not merge it — just pr ready {pr['number']} once it is reviewed",
        "behind": f"main moved under it — just pr merge {pr['number']} lands it as it stands, or just pr update {pr['number']}",
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


def rerun_waits(pr):
    """The gate's jobs still waiting on this head. The review job is left out: a round runs for hours."""
    jobs = jobs_of(pr)
    return [job for job in (*required(), UNREAD) if jobs.get(job) == "pending"]


def cmd_rerun(args):
    number = pull_number(args.number)
    # Incident: a second reopen on #1887 cancelled the run still waiting and wrote failure on every context.
    if waiting := rerun_waits(pull(number)):
        print(f"#{number}: a run is already waiting on {', '.join(waiting)}; a reopen would cancel it")
        return 1
    subprocess.run(("fgj", "pr", "close", *scope(), str(number)), capture_output=True, check=False)
    time.sleep(2)
    subprocess.run(("fgj", "pr", "reopen", *scope(), str(number)), capture_output=True, check=False)
    print(f"#{number} closed and reopened; the gate reruns")
    return 0


def queue_mode():
    """The queue's switch as the tick reads it: the forge's repository variable, and the environment only
    when that read fails, because a workflow exports the variable to the tick and a shell has nothing."""
    answer = fgj_api("GET", f"repos/{repo()}/actions/variables/MERGE_QUEUE")
    mode = answer.get("data") if isinstance(answer, dict) else None
    if mode in ("off", "shadow", "on"):
        return mode
    why = answer.get("message") if isinstance(answer, dict) else "no answer"
    told = os.environ.get("MERGE_QUEUE") or "off"
    print(f"merge: the forge did not read MERGE_QUEUE ({why}), so the environment says {told}")
    return told


def unreviewed(number, pr):
    """What the rounds still owe before a merge; nothing while rounds only report (shadow)."""
    import pr_review

    if pr_review.MODE != "blocking":
        return []
    rounds = pr_review.rounds_of(pr_review.comments(repo(), number), pr_review.authors(), number)
    return pr_review.ready_problems(rounds, pr["head"]["sha"], pr_review.holds_for(number, pr["base"]["ref"]))


def cmd_merge(args):
    number = pull_number(args.number)
    if getattr(args, "now", False):
        import pr_queue

        return pr_queue.cmd_merge_now(number)
    mode = queue_mode()
    need = required()
    deadline = time.monotonic() + args.timeout * 60
    while True:
        pr = pull(number)
        jobs = jobs_of(pr)
        verdict = decide(pr, jobs, need, is_behind(pr))
        if verdict == "behind" and pr.get("mergeable") is not False and mode != "on":
            # Invariant: main's rule no longer blocks an outdated branch, so a clean behind PR merges as it stands.
            verdict = decide(pr, jobs, need, False)
        if verdict == "merged":
            print(f"#{number} merged")
            return 0
        if verdict == "behind":
            if pr.get("mergeable") is False:
                print(f"#{number} conflicts with main; updating to surface it")
                if cmd_update(argparse.Namespace(number=number)):
                    print(f"merge: stopped — the forge refused to update #{number}; merge origin/main by hand")
                    return 1
            else:
                import pr_queue

                print(f"#{number} is behind main without a conflict; queueing it")
                return pr_queue.cmd_queue(argparse.Namespace(number=number), mode)
        elif verdict == "green":
            if errs := unreviewed(number, pr):
                for err in errs:
                    print(f"  {err}")
                print(f"merge: refused — #{number} needs two review rounds clean on its head")
                return 1
            # The head the gates were read on: the forge refuses a head that moved since.
            answer = fgj_api("POST", f"repos/{repo()}/pulls/{number}/merge",
                             {"Do": "merge", "head_commit_id": pr["head"]["sha"]})
            if answer and answer.get("message"):
                print(f"merge refused: {answer['message']}")
                if "behind" not in answer["message"]:
                    return 1
                if mode != "on":
                    # Incident: with main's rule back on, this POST was re-posted every POLL until the
                    # deadline, so the verb never reached the update that could resolve the refusal.
                    print(f"merge: the forge wants #{number} up to date first; updating it")
                    return cmd_update(argparse.Namespace(number=number))
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
    cmd_ratchet(argparse.Namespace(topic=args.title, no_binary=False))
    code = cmd_open(args)
    if code:
        return code
    # A PR lands through its review rounds: until two are clean on its head it stays a draft.
    args.number = None
    if cmd_ready(args):
        return 1
    args.wait = True
    return cmd_merge(args)


def selfcheck():
    # The bots' budgets read the workflows' own limit; a workflow that loses it fails here, not mid-run.
    assert job_secs("review") > 0 and job_secs("autofix") > 0
    try:
        job_secs("does-not-exist")
        raise AssertionError("a missing workflow read as a limit")
    except FileNotFoundError:
        pass
    # The listing pages: 102 open pulls are three pages, and a reader of page one never sees the oldest.
    served = [{"number": n, "head": {"ref": f"b{n}"}} for n in range(102)]

    def listing(method, path, payload=None):
        if "pulls?state=open" not in path:
            return {"message": "not this one"}
        page = int(re.search(r"page=(\d+)", path).group(1))
        return served[(page - 1) * 50:page * 50]

    real = globals()["fgj_api"], globals()["repo"]
    globals()["fgj_api"], globals()["repo"] = listing, lambda: "o/r"
    try:
        assert [pr["number"] for pr in open_pulls()] == list(range(102)), "the open listing skipped a page's worth"
        assert pull_for_branch("b101")["number"] == 101, "a pull request past the first page is not found by branch"
    finally:
        globals()["fgj_api"], globals()["repo"] = real
    globals()["fgj_api"] = lambda *a, **k: {"message": "no token"}
    try:
        assert open_pulls()["message"] == "no token", "an error object read as a page"
    finally:
        globals()["fgj_api"] = real[0]
    # A page after the first answering with an error object is returned, never spliced into the list.
    globals()["fgj_api"] = lambda m, path, payload=None: served[:50] if path.endswith("page=1") else {"message": "no token"}
    try:
        late = open_pulls()
        assert not isinstance(late, list) and late["message"] == "no token", f"a later page's error object read as {late[50:]!r}"
    finally:
        globals()["fgj_api"] = real[0]
    for gone, where in ((lambda *a, **k: None, "first"), (lambda m, path, payload=None: served[:50] if path.endswith("page=1") else None, "later")):
        globals()["fgj_api"] = gone
        try:
            assert open_pulls() is None, f"a 404 on the {where} page read as a listing"
        finally:
            globals()["fgj_api"] = real[0]
    globals()["fgj_api"] = lambda *a, **k: served[:50]
    try:
        assert len(open_pulls()) == 50, "a forge that ignores `page` served its first page forever"
    finally:
        globals()["fgj_api"] = real[0]
    assert repo_of("ssh://git@forge.example.invalid:2222/apex/yi.git") == "apex/yi"
    assert repo_of("https://git.example.invalid/apex/yi") == "apex/yi"
    assert repo_of("git@github.com:jackmaxwil/yi.git") == "jackmaxwil/yi"
    assert required_jobs(["review / review (pull_request_target)"]) == ["review"], "the review job runs on the target event"
    assert required_jobs(["pr / gate (test) (pull_request)", "pr / title (pull_request)"]) == [
        "gate (test)", "title",
    ]
    jobs = {"title": "success", "gate (test)": "success"}
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
    def fake_measure(measured):
        """Replaces check_pr_metadata.measure with one returning `measured`; returns (the saved measure, the body with every REQUIRED section)."""
        saved = gate.measure
        gate.measure = lambda: (measured, None)
        return saved, "".join(f"## {title}\n\nwords\n\n" for title in gate.REQUIRED)


    real_repo = globals()["repo"]
    real_measure, filled = fake_measure(([], [], 0, ([], ["tool:bash"])))
    globals()["repo"] = lambda: "apex/yi"
    try:
        errs = check_problems("Lock the tool surface", filled)
        drafted = check_problems(DRAFT + "Lock the tool surface", "## Summary\n\nwords\n")
    finally:
        gate.measure, globals()["repo"] = real_measure, real_repo
    assert len(errs) == 1 and "## Claims ledger" in errs[0] and "tool:bash" in errs[0], errs
    assert not [e for e in drafted if e.startswith("title:")], "a draft title passes the title rule"
    assert any("no `## Why needed`" in e for e in drafted), "`just pr open` runs the template check"
    # Incident: the D249 hard-cap line missed the ratchet regex, so `just push` went on.
    import contextlib, io, types, check_binary_size

    over = check_binary_size.problems(9_500_000, check_binary_size.HARD_CAP_BYTES)[0]
    assert over == "dist binary 9500000 > hard cap 9437184 bytes", over
    capped = f"FAIL binary_size\n  {over}\n"
    real_run = subprocess.run
    subprocess.run = lambda cmd, **kw: subprocess.CompletedProcess(cmd, int("check_binary" in cmd[-1]), capped, "")
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            stopped = binary_ratchet("")
    finally:
        subprocess.run = real_run
    assert stopped == 1, "a hard-cap breach stops the push lane"
    # One byte past the jitter allowance is a lone overrun, so the push ratchets the measured size.
    jitter = check_binary_size.JITTER
    assert overrun(f"FAIL binary_size\n  {check_binary_size.problems(800_001 + jitter, 800_000, name='binary_size_budget_linux.json')[0]}\n") == (
        "binary_size_budget_linux", 800_000, 800_001 + jitter)
    import pr_review

    real_fgj, real_pull = globals()["fgj_api"], globals()["pull"]
    real_rounds = pr_review.comments, pr_review.authors
    globals()["pull"] = lambda n: {"number": n, "title": DRAFT + "Keep the gate", "head": {"sha": "abc1234"}, "base": {"ref": "main"}}
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
    fakes = {name: globals()[name] for name in ("pull", "jobs_of", "required", "is_behind", "decide", "unreviewed", "fgj_api")}
    def forge_variable(mode, sink=None, merge=None):
        """A forge whose MERGE_QUEUE answers `mode` (None: the token may not read it); other calls land in `sink`."""
        def forge(method, path, payload=None):
            if "actions/variables" in path:
                return {"message": "403 the token may not read variables"} if mode is None else {"name": "MERGE_QUEUE", "data": mode}
            if sink is not None:
                sink.append(path)
            return (merge or {}) if path.endswith("/merge") else {}
        return forge

    posted = []
    globals().update(pull=lambda n: {"number": n, "title": "Keep the gate", "head": {"sha": "abc1234"}, "base": {"ref": "main"}},
                     jobs_of=lambda pr: {}, required=lambda: [], is_behind=lambda pr: False, decide=lambda *a: "green",
                     unreviewed=lambda n, pr: ["0 review round(s); a PR needs two"],
                     fgj_api=forge_variable("off", posted))
    try:
        with contextlib.redirect_stdout(io.StringIO()) as merging:
            held = cmd_merge(argparse.Namespace(number=7, timeout=1, wait=False))
    finally:
        globals().update(fakes)
    assert held == 1 and not posted and "needs two review rounds" in merging.getvalue(), "a green PR without rounds is not merged"
    globals().update(pull=lambda n: {"number": n, "title": "Keep the gate", "mergeable": False,
                                     "head": {"sha": "abc1234"}, "base": {"ref": "main"}},
                     jobs_of=lambda pr: {}, required=lambda: [], is_behind=lambda pr: True, decide=lambda *a: "behind",
                     fgj_api=lambda method, path, payload=None: {"message": "merge conflict"})
    try:
        with contextlib.redirect_stdout(io.StringIO()) as conflicted:
            refused = cmd_merge(argparse.Namespace(number=7, timeout=0, wait=True))
    finally:
        globals().update(fakes)
    assert refused == 1 and "refused to update" in conflicted.getvalue(), "a refused update stops the merge loop"
    import os, pr_queue
    from unittest import mock

    # The switch is the forge's repository variable: queue.yml exports it to the tick, and a shell has nothing.
    globals().update(fgj_api=forge_variable("on"))
    try:
        with mock.patch.dict(os.environ, {"MERGE_QUEUE": "off"}), contextlib.redirect_stdout(io.StringIO()):
            told = queue_mode()
    finally:
        globals().update(fakes)
    assert told == "on", f"the queue's switch was read from the shell while the forge said on: {told}"
    globals().update(fgj_api=forge_variable(None))
    try:
        with mock.patch.dict(os.environ, {"MERGE_QUEUE": "shadow"}), contextlib.redirect_stdout(io.StringIO()) as unread:
            told = queue_mode()
    finally:
        globals().update(fakes)
    assert told == "shadow" and "403 the token may not read variables" in unread.getvalue() and "shadow" in unread.getvalue(), (
        f"a forge that cannot answer MERGE_QUEUE fell back without saying which it used: {told}, {unread.getvalue()!r}")

    behind = {"number": 7, "title": "Keep the gate", "mergeable": True, "head": {"sha": "abc1234"}, "base": {"ref": "main"}}
    queued = []
    globals().update(pull=lambda n: behind, jobs_of=lambda pr: {}, required=lambda: [], is_behind=lambda pr: True,
                     decide=lambda pr, jobs, need, is_behind: "behind" if is_behind else "green", unreviewed=lambda n, pr: [],
                     fgj_api=forge_variable("on", queued))
    try:
        with mock.patch.dict(os.environ, {"MERGE_QUEUE": "off"}), \
                mock.patch.object(pr_queue, "cmd_queue", lambda args, mode: queued.append((args.number, mode)) or 0), \
                contextlib.redirect_stdout(io.StringIO()):
            clean = cmd_merge(argparse.Namespace(number=7, timeout=0, wait=False))
    finally:
        globals().update(fakes)
    assert clean == 0 and queued == [(7, "on")], (
        f"a behind, clean PR did not reach the queue the forge says is on: exit={clean}, calls={queued}")
    merged = []
    globals().update(pull=lambda n: {"number": n, "title": "Keep the gate", "mergeable": True, "merged": bool(merged),
                                     "state": "open", "head": {"sha": "abc1234"}, "base": {"ref": "main"}},
                     jobs_of=lambda pr: {}, required=lambda: [], is_behind=lambda pr: True, unreviewed=lambda n, pr: [],
                     decide=lambda pr, jobs, need, behind: "merged" if pr["merged"] else "behind" if behind else "green",
                     fgj_api=forge_variable("off", merged))
    try:
        with mock.patch.dict(os.environ, {"MERGE_QUEUE": "on"}), contextlib.redirect_stdout(io.StringIO()):
            cmd_merge(argparse.Namespace(number=7, timeout=0, wait=False))
    finally:
        globals().update(fakes)
    assert merged and merged[-1].endswith("pulls/7/merge"), (
        f"a behind, clean PR was queued for a forge whose variable is off: {merged}")
    updated = []
    globals().update(pull=lambda n: {"number": n, "title": "Keep the gate", "mergeable": True, "merged": False,
                                     "state": "open", "head": {"sha": "abc1234"}, "base": {"ref": "main"}},
                     jobs_of=lambda pr: {}, required=lambda: [], is_behind=lambda pr: False,
                     decide=lambda pr, jobs, need, behind: "green", unreviewed=lambda n, pr: [],
                     fgj_api=forge_variable("off", updated, merge={"message": "This branch is behind main; the merge was refused"}))
    try:
        with mock.patch.dict(os.environ, {"MERGE_QUEUE": "off"}), contextlib.redirect_stdout(io.StringIO()):
            code = cmd_merge(argparse.Namespace(number=7, timeout=0, wait=True))
    finally:
        globals().update(fakes)
    assert code == 0 and [path.rsplit("/pulls/7/", 1)[-1] for path in updated] == ["merge", "update?style=merge"], (
        f"a green merge the forge refused as out of date never reached the update: exit={code}, calls={updated}")

    def merge_on_fake_forge(protected, statuses):
        """cmd_merge for #7 against a forge whose rule lists `protected` and whose head `abc1234` ran
        `statuses`: (exit code, the bodies posted to /merge)."""
        posts = []

        def forge(method, path, payload=None):
            if path.endswith("/merge"):
                posts.append(payload)
                return {}
            if path.endswith("/branch_protections"):
                return [{"branch_name": "main", "status_check_contexts": protected}]
            if path.endswith("/commits/abc1234/status"):
                return {"statuses": [{"context": f"pr / {job} (pull_request)", "status": status} for job, status in statuses.items()]}
            return {"number": 7, "title": "Keep the gate", "state": "closed" if posts else "open", "merged": bool(posts),
                    "head": {"sha": "abc1234", "ref": "x"}, "base": {"ref": "main"}}

        saved = {name: globals()[name] for name in ("fgj_api", "repo", "is_behind", "unreviewed", "POLL")}
        globals().update(fgj_api=forge, repo=lambda: "apex/yi", is_behind=lambda pr: False,
                         unreviewed=lambda n, pr: [], POLL=0)
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                code = cmd_merge(argparse.Namespace(number=7, timeout=1, wait=False))
        finally:
            globals().update(saved)
        return code, posts

    green = {"gate (guardrails)": "success", "gate (lint)": "success", "gate (test)": "success", "title": "success"}
    listed = ["pr / title (pull_request)"]
    code, posts = merge_on_fake_forge(listed, green)
    assert code == 0 and posts == [{"Do": "merge", "head_commit_id": "abc1234"}], ("the merge names the head the gates ran on", posts)
    code, posts = merge_on_fake_forge(listed, dict(green, **{"gate (test)": "failure"}))
    assert code == 1 and not posts, "a red gate (test) on the head is not merged even when the rule does not list it"
    code, posts = merge_on_fake_forge(listed, {job: s for job, s in green.items() if job != "gate (lint)"})
    assert code == 2 and not posts, "a gate that has not run on the head is not merged"
    assert "0 review round(s)" in said.getvalue(), "ready says what the rounds still owe"
    assert required_jobs(["pr / gate (test) (push)", "bench-smoke / smoke (pull_request)"]) == ["gate (test)", "smoke"]
    # The real order on f84b43871: failure, then the reopen's pending, while no runner has claimed a job.
    head_status = {"statuses": [{"context": "pr / gate (test) (pull_request)", "status": "pending"},
                           {"context": "pr / gate (lint) (pull_request)", "status": "error"},
                           {"context": "review / review (pull_request_target)", "status": "pending"}]}
    saved_forge = globals()["fgj_api"], globals()["repo"], globals()["pull"], subprocess.run
    closed = []
    globals().update(fgj_api=lambda *a, **k: head_status, repo=lambda: "o/r", pull=lambda n: {"number": n, "head": {"sha": "abc1234"}})
    subprocess.run = lambda cmd, **kw: closed.append(cmd)
    try:
        seen = jobs_of({"head": {"sha": "abc1234"}})
        assert seen.get("gate (test)") == "pending" and seen.get("gate (lint)") == "failure", ("a waiting run read as its old failure", seen)
        with contextlib.redirect_stdout(io.StringIO()):
            assert cmd_rerun(argparse.Namespace(number=7)) == 1 and not closed, "a second reopen cancels the waiting run"
            served, head_status = head_status, {"message": "forbidden"}
            assert cmd_rerun(argparse.Namespace(number=7)) == 1 and not closed, "an unreadable status lets a reopen cancel a waiting run"
            head_status = served
            head_status["statuses"][0] = {"context": "pr / gate (test) (pull_request)", "status": "failure"}
            real_sleep, time.sleep = time.sleep, lambda s: None
            try:
                assert cmd_rerun(argparse.Namespace(number=7)) == 0 and len(closed) == 2, "a pending review holds a rerun back"
            finally:
                time.sleep = real_sleep
    finally:
        subprocess.run = saved_forge[3]
        globals().update(fgj_api=saved_forge[0], repo=saved_forge[1], pull=saved_forge[2])
    # An error object from a token without admin rights, and a rule naming the review job.
    for answer, want in (({"message": "forbidden"}, ["title"]), (None, ["title"]),
                         ([{"branch_name": "main", "status_check_contexts": ["pr / title (pull_request)", "review / review (pull_request_target)"]}], ["title"])):
        globals().update(fgj_api=lambda *a, **k: answer, repo=lambda: "o/r")
        try:
            assert required() == [*pr_review.GATE_LANES, *want], ("required()", answer, required())
        finally:
            globals().update(fgj_api=saved_forge[0], repo=saved_forge[1])
    real_measure, _ = fake_measure(([], [], 0, ([], [])))
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
    gate.measure = lambda: (([], [], 200, ([], [])), None)
    prefill = "## UI changes\n\nNone.\n"
    unlisted = "".join(f"## {title}\n\nwords\n\n" for title in gate.REQUIRED if title != "UI changes")
    body_file = pathlib.Path(tempfile.gettempdir()) / "forge_pr_selfcheck_check.md"
    body_file.write_text(unlisted)
    issue_ok, judged, real_check, real_run = gate.issue_problems, [], check_problems, subprocess.run
    gate.issue_problems = lambda *a: []
    globals()["check_problems"] = lambda title, body: judged.append(body) or real_check(title, body)
    subprocess.run = lambda cmd, *a, **k: types.SimpleNamespace(stdout=prefill) if "pr_body.py" in str(cmd) else real_run(cmd, *a, **k)
    try:
        for closes, want in (([1], 0), ([], 1)):
            with contextlib.redirect_stdout(io.StringIO()):
                code = cmd_check(argparse.Namespace(number=None, title="Keep the gate", body=str(body_file), closes=closes, refs=[]))
            assert code == want, ("pr check judges the body as pr open composes it", closes, code)
    finally:
        gate.measure, gate.issue_problems, subprocess.run = real_measure, issue_ok, real_run
        globals()["check_problems"] = real_check
        body_file.unlink()
    assert judged[0] == "Closes #1\n\n" + unlisted.strip() + "\n\n" + prefill.strip(), ("pr check judges the body pr open submits", judged[0])
    # Incident: a merge resolution dropped the check verb's flags and every gate stayed green;
    # the parser is built and every wired name resolved here, so neither repeats.
    flags = {}
    try:
        flags = vars(build_parser().parse_args(["pr", "check", "--closes", "1", "--refs", "2"]))
    except SystemExit:
        pass
    assert flags.get("closes") == ["1"] and flags.get("refs") == ["2"], ("pr check takes the flags pr open takes", flags)
    # Incident: review round 1's fixer deleted `def cmd_commit` and every hook stayed green.
    commit_selfcheck()
    print("ok   forge_pr selfcheck")


def commit_selfcheck():
    """`commit` in a scratch repository holding one named and one unnamed untracked file."""
    import contextlib, io, os

    names = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
    saved = {name: globals()[name] for name in ("ROOT", "baseline_paths", "binary_ratchet")}
    with tempfile.TemporaryDirectory() as scratch:
        os.environ.update(names)
        subprocess.run(("git", "-C", scratch, "init", "-q"), check=True)
        globals().update(ROOT=pathlib.Path(scratch), baseline_paths=lambda: [], binary_ratchet=lambda topic: 0)
        parse = build_parser().parse_args
        try:
            (pathlib.Path(scratch) / "a.txt").write_text("a\n")
            (pathlib.Path(scratch) / "b.txt").write_text("b\n")
            with contextlib.redirect_stdout(io.StringIO()) as said:
                refused = cmd_commit(parse(["commit", "Add the files"]))
            assert refused == 1 and "b.txt" in said.getvalue() and not git("log", "--oneline"), ("nothing named, nothing staged", said.getvalue())
            with contextlib.redirect_stdout(io.StringIO()):
                done = cmd_commit(parse(["commit", "Add the named file", "a.txt"]))
            assert done == 0 and git("show", "--name-only", "--format=", "HEAD") == "a.txt", git("show", "--name-only", "--format=", "HEAD")
            assert git("status", "--porcelain") == "?? b.txt", "a path the caller did not name stays out of the commit"
            git("add", "--", "b.txt", check=True)
            (pathlib.Path(scratch) / "c.txt").write_text("c\n")
            with contextlib.redirect_stdout(io.StringIO()):
                done = cmd_commit(parse(["commit", "Add the third file", "c.txt"]))
            assert done == 0 and git("show", "--name-only", "--format=", "HEAD") == "c.txt", "a file staged by someone else rides into a named-path commit"
            assert git("diff", "--cached", "--name-only") == "b.txt", "the unrelated staged file stays staged"
            with contextlib.redirect_stdout(io.StringIO()):
                done = cmd_commit(parse(["commit", "Add the staged file"]))
            assert done == 0 and git("show", "--name-only", "--format=", "HEAD") == "b.txt", "no paths named commits what is staged"
        finally:
            globals().update(saved)
            for name in names:
                del os.environ[name]


def build_parser():
    parser = argparse.ArgumentParser(prog="just")
    verbs = parser.add_subparsers(dest="verb", required=True)
    ratchet = verbs.add_parser("ratchet")
    ratchet.add_argument("topic", nargs="?", default="")
    ratchet.add_argument("--no-binary", action="store_true", help="skip the dist build")
    ratchet.set_defaults(run=cmd_ratchet)
    commit = verbs.add_parser("commit")
    commit.add_argument("subject")
    commit.add_argument("paths", nargs="*")
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
    check.add_argument("--closes", action="append", default=[])
    check.add_argument("--refs", action="append", default=[])
    check.set_defaults(run=cmd_check)
    edit = pr.add_parser("edit")
    edit.add_argument("number", type=int)
    edit.add_argument("--title")
    edit.add_argument("--body")
    edit.set_defaults(run=cmd_edit)
    import pr_review

    for name, run in (("status", cmd_status), ("ready", cmd_ready), ("update", cmd_update), ("rerun", cmd_rerun),
                      ("review", pr_review.cmd_review)):
        sub = pr.add_parser(name)
        sub.add_argument("number", nargs="?")
        sub.set_defaults(run=run)
    pr.choices["review"].add_argument("--dry-run", action="store_true", help="print the round, post nothing")
    pr.choices["review"].add_argument("--again", action="store_true", help="read a head the rule says is read enough, except one a round blocked unless that round lost a lens")
    pr.add_parser("sweep").set_defaults(run=pr_review.cmd_sweep)
    import pr_autofix

    autofix = pr.add_parser("autofix", help="fix conflicts with the base: one PR now, or one pass oldest first")
    autofix.add_argument("number", nargs="?")
    autofix.set_defaults(run=pr_autofix.cmd_autofix)
    import bot_meter

    spend = pr.add_parser("spend", help="what the review and autofix bots spent, from their comments")
    spend.add_argument("--days", type=int, default=1)
    spend.set_defaults(run=bot_meter.cmd_spend)
    replay = pr.add_parser("replay")
    replay.add_argument("numbers", nargs="+", type=int)
    replay.add_argument("--label", required=True, help="what the PR is known to be: bad, kept, closed")
    replay.add_argument("--out", required=True, help="the JSON-lines file each reading is appended to")
    replay.set_defaults(run=pr_review.cmd_replay)
    calibrate = pr.add_parser("calibrate", help="precision and recall of a high finding on replay files")
    calibrate.add_argument("files", nargs="+")
    calibrate.set_defaults(run=pr_review.cmd_calibrate)
    merge = pr.add_parser("merge")
    merge.add_argument("number", nargs="?")
    merge.add_argument("--no-wait", dest="wait", action="store_false")
    merge.add_argument("--timeout", type=int, default=40)
    merge.add_argument("--now", action="store_true", help="the owner's explicit ask: force the merge, past the queue")
    merge.set_defaults(run=cmd_merge)
    import pr_queue

    queue = pr.add_parser("queue", help="add a reviewed PR to the merge queue; no number prints the queue")
    queue.add_argument("number", nargs="?")
    queue.set_defaults(run=pr_queue.cmd_queue)
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
