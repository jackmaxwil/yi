#!/usr/bin/env python3
"""The merge queue's tick: gate a PR merged onto current main, then fast-forward main to that commit.

    pr_queue.py               one tick, in the mode the MERGE_QUEUE variable names (shadow, on)
    pr_queue.py --selfcheck   every rule with each one disabled in turn, and the git steps for real

Nothing is stored: the state is the `queue` label, the `queue/candidate` ref and that ref's statuses,
so a crashed tick loses nothing. A red candidate stays until it is replaced, since it is the memory
of what comes next. A retry's count rides the tip's body. Recovery is deleting the ref (docs/FORGE.md)."""
import argparse, contextlib, io, os, pathlib, re, shutil, subprocess, sys, tempfile, time

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "scripts"), str(ROOT / "scripts/guardrails")]
import forge_pr, forge_tracking, pr_review
from check_pr_metadata import CITE

LABEL, CANDIDATE = "queue", "queue/candidate"
# Sessions sign in as `jack` too, so an enqueue only when the owner asked stays a rule of conduct.
MERGERS = {"jack"}
K = 1
# Invariant: the slowest green gate lane is about an hour on a loaded runner, and a run that has not
# reported by then is lost, because a candidate whose pr.yml runs no lanes has no statuses at all.
STALE_MINUTES = 80
INFRA_RETRIES = 2
IDENTITY = ("-c", "user.name=yi-queue", "-c", "user.email=queue@yi")
LANES = pr_review.GATE_LANES
RETRY = re.compile(r"^Queue retry (\d+)\.$", re.M)
MEMBER = re.compile(r"\(#(\d+)\) from ")


def lanes_state(obs):
    states = [obs["lanes"].get(lane) for lane in LANES]
    # Invariant: on a candidate, a missing lane is pending, including the seconds before a status exists.
    if all(state == "success" for state in states):
        return "green"
    return "pending" if any(state not in ("success", "failure") for state in states) else "red"


def stale(obs):
    return obs["age_minutes"] >= STALE_MINUTES


def outage(obs):
    return all(obs["lanes"].get(lane) == "failure" for lane in LANES)


def infra(obs):
    return (lanes_state(obs) == "pending" and stale(obs)) or outage(obs)


# Read top to bottom by step: the first rule that holds names the action. No candidate, then only the
# last two can hold. `red` is any failed lane once every lane has answered.
RULES = (
    ("landed", lambda o: o["candidate"] and o["in_main"], "finish"),
    ("base moved", lambda o: o["candidate"] and o["base"] != o["main"], "drop"),
    ("member moved", lambda o: o["candidate"] and o["stale_member"], "drop"),
    ("waiting", lambda o: o["candidate"] and lanes_state(o) == "pending" and not stale(o), "wait"),
    ("green", lambda o: o["candidate"] and lanes_state(o) == "green", "land"),
    ("infra, retries left", lambda o: o["candidate"] and infra(o) and o["retries"] < INFRA_RETRIES, "retry"),
    ("infra, retries spent", lambda o: o["candidate"] and infra(o), "paused"),
    ("red, first time", lambda o: o["candidate"] and o["retries"] == 0, "retry"),
    ("red, retried", lambda o: o["candidate"], "eject"),
    ("queue waiting", lambda o: o["queued"], "build"),
    ("nothing to do", lambda o: True, "idle"),
)


def step(obs, off=""):
    return next(((action, name) for name, holds, action in RULES if name != off and holds(obs)), ("none", "none"))


def git(*args, cwd=ROOT, check=True):
    out = subprocess.run(("git", "-C", str(cwd), *args), capture_output=True, text=True, errors="replace", check=False)
    if check and out.returncode:
        raise SystemExit(f"queue: git {' '.join(args)}: {out.stderr.strip()}")
    return out


def members_of(tip, base):
    """(PR number, its head) per merge on the first-parent walk from `base` to `tip`, oldest first."""
    log = git("log", "--first-parent", "--reverse", "--format=%P%x00%s", f"{base}..{tip}").stdout
    found = []
    for line in filter(None, log.splitlines()):
        parents, subject = line.split("\x00")
        number = MEMBER.search(subject)
        if number and len(parents.split()) == 2:
            found.append((int(number.group(1)), parents.split()[1]))
    return found


def merge_member(cwd, number, head, title, branch, body=""):
    """Merge one PR's head under the forge's own subject, so the recorder and the mirror read it unchanged."""
    subject = f"Merge pull request '{title}' (#{number}) from {branch} into main"
    closes = "\n".join(f"Closes #{n}" for n in closing(body))
    message = subject + (f"\n\n{closes}" if closes else "")
    done = git(*IDENTITY, "merge", "--no-ff", "-m", message, head, cwd=cwd, check=False)
    if done.returncode == 0:
        return []
    left = git("diff", "--name-only", "--diff-filter=U", cwd=cwd).stdout.split()
    git("merge", "--abort", cwd=cwd, check=False)
    return left or [done.stderr.strip()[:200]]


def closing(body):
    return list(dict.fromkeys(int(n) for word, named, n in CITE.findall(body or "") if word.lower() == "closes" and not named))


def use_driver(cwd):
    git("config", "merge.baseline.driver", f"{sys.executable} {ROOT / 'scripts/merge_baseline.py'} %O %A %B", cwd=cwd)


def same_tree(base, tip, recipe):
    """Rebuild the merges in a scratch tree: only reviewed bytes land, whatever the ref now holds."""
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="yi-queue-"))
    try:
        git("worktree", "add", "-q", "--detach", str(tmp / "w"), base)
        use_driver(tmp / "w")
        for number, head, title, branch in recipe:
            if merge_member(tmp / "w", number, head, title, branch):
                return False
        return git("rev-parse", "HEAD^{tree}", cwd=tmp / "w").stdout == git("rev-parse", f"{tip}^{{tree}}").stdout
    finally:
        git("worktree", "remove", "--force", str(tmp / "w"), check=False)
        shutil.rmtree(tmp, ignore_errors=True)


def api(method, path, payload=None):
    return forge_pr.fgj_api(method, f"repos/{forge_pr.repo()}/{path}", payload)


def label_ids():
    return {label["name"]: label["id"] for label in api("GET", "labels?limit=100") or []}


def set_label(number, name, present):
    ids = label_ids()
    if name not in ids:
        raise SystemExit(f"queue: no `{name}` label on {forge_pr.repo()}")
    if present:
        api("POST", f"issues/{number}/labels", {"labels": [ids[name]]})
    else:
        api("DELETE", f"issues/{number}/labels/{ids[name]}")


def comment(number, text):
    api("POST", f"issues/{number}/comments", {"body": text})


def delete_ref():
    api("DELETE", "branches/" + CANDIDATE.replace("/", "%2F"))


def enqueue_time(events):
    """When a merger last added the label; None when nobody allowed did, or it has since come off.
    The forge lets any writer add one, and the timeline's body is "1" for an add."""
    at = None
    for event in events:
        if event.get("type") == "label" and (event.get("label") or {}).get("name") == LABEL:
            allowed = (event.get("user") or {}).get("login") in MERGERS
            at = event.get("created_at") if event.get("body") == "1" and allowed else None
    return at


def enqueued_at(number):
    transport = lambda method, url, _: forge_pr.fgj_api(method, url)
    return enqueue_time(forge_tracking.paged(transport, f"repos/{forge_pr.repo()}/issues/{number}/timeline?x=1"))


def queued_pulls():
    return [pr for pr in forge_pr.open_pulls() if LABEL in {label["name"] for label in pr.get("labels") or []}]


def eligible_problem(pr, jobs):
    labels = {label["name"] for label in pr.get("labels") or []}
    if pr["base"]["ref"] != "main":
        return f"based on {pr['base']['ref']}, not main"
    if labels & {"autofix", "autofix:working"}:
        return "the fixer holds it"
    verdict = forge_pr.decide(pr, jobs, [*LANES, "title"], False)
    if verdict != "green":
        return f"its own gate reads {verdict}"
    if errs := forge_pr.unreviewed(pr["number"], pr):
        return errs[0].strip()
    return None


def dequeue_reason(pr):
    if "autofix:failed" in {label["name"] for label in pr.get("labels") or []}:
        return "the fixer gave up on it (`autofix:failed`)"
    rounds = pr_review.rounds_of(pr_review.comments(forge_pr.repo(), pr["number"]), pr_review.authors(), pr["number"])
    if rounds and rounds[-1].get("verdict") == "blocked":
        return "its last review round is blocked"
    return None


def shadow_note(candidate, head, base):
    return f"Shadow: the queue would land {candidate[:10]}, with this PR at {head[:10]} on main {base[:10]}."


def already_said(number, sha):
    return any("would land" in (c.get("body") or "") and sha[:10] in c.get("body", "")
               for c in pr_review.comments(forge_pr.repo(), number))


def order(pulls):
    """Oldest enqueue first; a label nobody allowed added leaves the PR out."""
    dated = [(at, pr) for pr in pulls if (at := enqueued_at(pr["number"]))]
    return [pr for _, pr in sorted(dated, key=lambda pair: pair[0])]


def observe(mode):
    git("fetch", "-q", "origin", "main")
    pulls = queued_pulls()
    for pr in list(pulls):
        if reason := dequeue_reason(pr):
            set_label(pr["number"], LABEL, False)
            comment(pr["number"], f"Left the queue: {reason}. Add the `{LABEL}` label again once that is answered.")
            pulls.remove(pr)
    obs = {"candidate": None, "in_main": False, "base": None, "main": git("rev-parse", "origin/main").stdout.strip(),
           "stale_member": None, "lanes": {}, "age_minutes": 0, "retries": 0, "members": [], "queued": [], "pulls": pulls}
    if git("ls-remote", "origin", f"refs/heads/{CANDIDATE}").stdout.strip():
        git("fetch", "-q", "origin", f"+refs/heads/{CANDIDATE}:refs/remotes/origin/{CANDIDATE}")
        tip = git("rev-parse", f"origin/{CANDIDATE}").stdout.strip()
        obs["candidate"] = tip
        obs["in_main"] = git("merge-base", "--is-ancestor", tip, "origin/main", check=False).returncode == 0
        obs["base"] = git("rev-parse", f"{tip}^1").stdout.strip()
        obs["members"] = members_of(tip, obs["base"])
        obs["lanes"] = forge_pr.head_jobs(forge_pr.repo(), tip)
        date = git("log", "-1", "--format=%ct", tip).stdout.strip()
        obs["age_minutes"] = (time.time() - int(date)) / 60
        body = git("log", "-1", "--format=%B", tip).stdout
        retry = RETRY.search(body)
        obs["retries"] = int(retry.group(1)) if retry else 0
        by_number = {pr["number"]: pr for pr in pulls}
        for number, head in obs["members"]:
            pr = by_number.get(number)
            if pr is None:
                obs["stale_member"] = f"#{number} left the queue"
            elif pr["head"]["sha"] != head:
                obs["stale_member"] = f"#{number}'s head moved"
            elif errs := forge_pr.unreviewed(number, pr):
                obs["stale_member"] = f"#{number}: {errs[0].strip()}"
    else:
        for pr in order(pulls):
            head = pr["head"]["sha"]
            problem = eligible_problem(pr, forge_pr.jobs_of(pr))
            if problem is None and git("merge-base", "--is-ancestor", head, "origin/main", check=False).returncode == 0:
                problem = "its head is already in main"
            if problem is None and mode == "shadow" and already_said(pr["number"], head):
                problem = "shadow already said it would land"
            if problem:
                print(f"queue: #{pr['number']} skipped: {problem}")
            else:
                obs["queued"].append(pr)
    return obs


def recipe_of(pr):
    return pr["number"], pr["head"]["sha"], pr["title"], pr["head"]["ref"]


def record_main(root=ROOT):
    """Record what main has landed; False only when main moved under the push."""
    git("fetch", "-q", "origin", "main", cwd=root)
    git("checkout", "-q", "--detach", "origin/main", cwd=root)
    subprocess.run((sys.executable, str(root / "scripts/record_changes.py")), cwd=root, check=True)
    if not git("status", "--porcelain", "docs", cwd=root).stdout.strip():
        return True
    git("add", "docs", cwd=root)
    git("-c", "user.name=yi-ci", "-c", "user.email=ci@yi", "commit", "-q", "--author=yi-ci <ci@yi>", "-m", "Record the change files main landed", cwd=root)
    pushed = git("push", "-q", "origin", "HEAD:main", cwd=root, check=False)
    if not pushed.returncode:
        return True
    git("fetch", "-q", "origin", "main", cwd=root)
    if git("rev-parse", "origin/main", cwd=root).stdout != git("rev-parse", "HEAD^", cwd=root).stdout:
        return False
    # Invariant: a candidate does not depend on the record, so a refusal that is not a moved main never stops the queue.
    print(f"queue: the recorder's push was refused with main unmoved: {pushed.stderr.strip()[:200]}")
    return True


def build(obs, mode):
    # Incident: the recorder's job waited on a runner and pushed while candidate d862f188 went green,
    # so a green gate was thrown away. A shadow candidate never lands, so it has nothing to protect.
    if mode == "on" and not record_main():
        print("queue: main moved under the recorder's push; the next tick builds")
        return
    git("checkout", "-q", "-B", CANDIDATE, "origin/main")
    use_driver(ROOT)
    built = []
    for pr in obs["queued"][:K]:
        git("fetch", "-q", "origin", f"refs/pull/{pr['number']}/head")
        if left := merge_member(ROOT, *recipe_of(pr), pr.get("body") or ""):
            # Invariant: the batch's first member merges against main alone, so its conflict is main's.
            names = ", ".join(f"`{path}`" for path in left)
            comment(pr["number"], f"The queue could not merge this onto main: {names}. The fixer takes it from here, and the label stays.")
            set_label(pr["number"], "autofix", True)
            continue
        built.append(pr["number"])
    if not built:
        print("queue: nothing merged, so no candidate")
        return
    git("push", "-q", "--force", "origin", f"HEAD:refs/heads/{CANDIDATE}")
    print(f"queue: candidate {git('rev-parse', 'HEAD').stdout.strip()[:10]} for {built}")


def retry(obs):
    tip, parents = obs["candidate"], git("log", "-1", "--format=%P", obs["candidate"]).stdout.split()
    body = RETRY.sub("", git("log", "-1", "--format=%B", tip).stdout).strip()
    message = f"{body}\n\nQueue retry {obs['retries'] + 1}."
    env = {**os.environ, "GIT_AUTHOR_NAME": "yi-queue", "GIT_AUTHOR_EMAIL": "queue@yi",
           "GIT_COMMITTER_NAME": "yi-queue", "GIT_COMMITTER_EMAIL": "queue@yi"}
    args = ["git", "-C", str(ROOT), "commit-tree", f"{tip}^{{tree}}", "-m", message]
    for parent in parents:
        args += ["-p", parent]
    made = subprocess.run(args, capture_output=True, text=True, env=env, check=True).stdout.strip()
    git("push", "-q", "--force", "origin", f"{made}:refs/heads/{CANDIDATE}")
    print(f"queue: retry {obs['retries'] + 1} as {made[:10]}")


def statuses_url(sha, lane):
    answer = api("GET", f"commits/{sha}/status") or {}
    for status in answer.get("statuses") or []:
        if forge_pr.required_jobs([status["context"]])[0] == lane and status.get("target_url"):
            return status["target_url"]
    return ""


def eject(obs):
    failed = [lane for lane in LANES if obs["lanes"].get(lane) == "failure"]
    for number, _ in obs["members"]:
        set_label(number, LABEL, False)
        where = statuses_url(obs["candidate"], failed[0]) if failed else ""
        comment(number, f"Left the queue: {', '.join(failed)} failed twice on this PR merged onto main, at "
                        f"{obs['candidate'][:10]}. {where}\nAdd the `{LABEL}` label again once it is answered.")
        # Without this the fixer sees a green head with no conflict and decides it is clean, so the
        # failure has to reproduce on the PR's own head.
        pr = forge_pr.pull(number)
        if forge_pr.is_behind(pr) and forge_pr.cmd_update(argparse.Namespace(number=number)):
            set_label(number, "autofix", True)
    delete_ref()


def finish(obs):
    """Idempotent, so a tick killed after the push does this through the `landed` rule instead."""
    for number, _ in obs["members"]:
        pr = forge_pr.pull(number)
        # The forge marks the PR but fires no closing keyword on a push (the 2026-11-01 probe).
        for issue in closing(pr.get("body")):
            api("PATCH", f"issues/{issue}", {"state": "closed"})
        set_label(number, LABEL, False)
    if not record_main():
        print("queue: the recorder's push was refused; the next push to main records")
    delete_ref()
    print(f"queue: landed {[n for n, _ in obs['members']]}")


def land(obs, mode):
    recipe = []
    for number, head in obs["members"]:
        pr = forge_pr.pull(number)
        recipe.append((number, head, pr["title"], pr["head"]["ref"], pr.get("body") or ""))
    if not same_tree(obs["base"], obs["candidate"], [r[:4] for r in recipe]):
        print("queue: the candidate's tree is not its merges; dropping it")
        return delete_ref()
    if mode == "shadow":
        for number, head in obs["members"]:
            comment(number, shadow_note(obs["candidate"], head, obs["base"]))
        return delete_ref()
    pushed = git("push", "-q", "origin", f"{obs['candidate']}:refs/heads/main", check=False)
    if pushed.returncode:
        print(f"queue: main refused the push, so the candidate goes: {pushed.stderr.strip()[:200]}")
        return delete_ref()
    finish(obs)


def tick(mode):
    obs = observe(mode)
    action, why = step(obs)
    print(f"queue: {action} ({why}){' ' + obs['stale_member'] if obs['stale_member'] else ''}")
    if action == "drop":
        delete_ref()
    elif action == "build":
        build(obs, mode)
    elif action == "retry":
        retry(obs)
    elif action == "eject":
        eject(obs)
    elif action == "land":
        land(obs, mode)
    elif action == "finish":
        finish(obs)
    elif action == "paused":
        print(f"queue: paused on {obs['candidate'][:10]}, whose lanes read {obs['lanes']}; deleting {CANDIDATE} resumes")
    return 0


def cmd_queue(args):
    """Add one PR, refusing what a tick would skip; with no number, print the queue."""
    if args.number is None:
        pulls = order(queued_pulls())
        for pr in pulls:
            print(f"#{pr['number']} {pr['title']}: {eligible_problem(pr, forge_pr.jobs_of(pr)) or 'ready'}")
        tip = git("ls-remote", "origin", f"refs/heads/{CANDIDATE}").stdout.split()
        if tip:
            print(f"candidate {tip[0][:10]}: {forge_pr.head_jobs(forge_pr.repo(), tip[0])}")
        return 0
    number = forge_pr.pull_number(args.number)
    pr = forge_pr.pull(number)
    if problem := eligible_problem(pr, forge_pr.jobs_of(pr)):
        print(f"queue: #{number} refused: {problem}")
        return 1
    set_label(number, LABEL, True)
    print(f"#{number} queued; a tick lands it")
    return 0


def cmd_merge_now(number):
    """The owner's "merge it now": lands on a possibly old base, so explicit asks only."""
    pr = forge_pr.pull(number)
    set_label(number, LABEL, False)
    answer = forge_pr.fgj_api("POST", f"repos/{forge_pr.repo()}/pulls/{number}/merge",
                              {"Do": "merge", "force_merge": True, "head_commit_id": pr["head"]["sha"]})
    if answer and answer.get("message"):
        print(f"merge refused: {answer['message']}")
        return 1
    print(f"#{number} merged now, past the queue and any gate or round still owed on {pr['head']['sha'][:8]}")
    return 0


OBS = {"candidate": "c", "in_main": False, "base": "m", "main": "m", "stale_member": None, "lanes": {},
       "age_minutes": 5, "retries": 0, "members": [(1, "h")], "queued": []}
GREEN = {lane: "success" for lane in LANES}
RED = {**GREEN, LANES[-1]: "failure"}
CASES = (
    ("landed", dict(in_main=True, lanes=RED), "finish"),
    ("base moved", dict(main="n", lanes=GREEN), "drop"),
    ("member moved", dict(stale_member="x", lanes=GREEN), "drop"),
    ("waiting", dict(lanes={LANES[0]: "pending"}), "wait"),
    ("waiting", dict(lanes={}), "wait"),
    ("green", dict(lanes=GREEN), "land"),
    ("infra, retries left", dict(lanes={}, age_minutes=STALE_MINUTES), "retry"),
    ("infra, retries left", dict(lanes={lane: "failure" for lane in LANES}), "retry"),
    ("infra, retries spent", dict(lanes={lane: "failure" for lane in LANES}, retries=INFRA_RETRIES), "paused"),
    ("red, first time", dict(lanes=RED), "retry"),
    ("red, retried", dict(lanes=RED, retries=1), "eject"),
    ("queue waiting", dict(candidate=None, queued=[{}]), "build"),
    ("nothing to do", dict(candidate=None), "idle"),
)


def rule_cases(off=""):
    bad = []
    for name, change, want in CASES:
        obs = {**OBS, **change}
        action, why = step(obs, off)
        if (action, why) != (want, name):
            bad.append(f"{name}: {change} read {action} ({why}), not {want}")
    return bad


def git_cases():
    """The merges, the rebuild and the push, against real git."""
    bad = []
    with tempfile.TemporaryDirectory(prefix="yi-queue-") as tmp:
        root = pathlib.Path(tmp) / "w"
        origin = pathlib.Path(tmp) / "origin.git"
        subprocess.run(("git", "init", "-q", "--bare", "-b", "main", str(origin)), check=True)
        subprocess.run(("git", "clone", "-q", str(origin), str(root)), capture_output=True, check=True)
        sh = lambda *a: git("-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *a, cwd=root, check=False)
        (root / "a.txt").write_text("one\n")
        sh("add", "."), sh("commit", "-qm", "seed"), sh("push", "-q", "origin", "HEAD:main")
        base = sh("rev-parse", "HEAD").stdout.strip()
        sh("checkout", "-q", "-b", "topic")
        (root / "b.txt").write_text("two ünï\n")
        sh("add", "."), sh("commit", "-qm", "topic")
        head = sh("rev-parse", "HEAD").stdout.strip()
        sh("checkout", "-q", "--detach", base)
        if left := merge_member(root, 7, head, "A title", "topic", "Closes #41\nRefs #42"):
            bad.append(f"a clean merge read conflicted: {left}")
        tip = sh("rev-parse", "HEAD").stdout.strip()
        body = sh("log", "-1", "--format=%B").stdout
        if "Closes #41" not in body or "#42" in body:
            bad.append(f"the merge's body carried the wrong citations: {body!r}")
        if not MEMBER.search(body.splitlines()[0]) or not body.startswith("Merge pull request 'A title' (#7) from topic into main"):
            bad.append(f"the merge's subject is not the forge's: {body!r}")
        rebuilt = sh("rev-parse", f"{tip}^{{tree}}").stdout
        sh("checkout", "-q", "--detach", base)
        if merge_member(root, 7, head, "A title", "topic") or sh("rev-parse", "HEAD^{tree}").stdout != rebuilt:
            bad.append("a rebuild of the same merge did not reach the same tree")
        sh("checkout", "-q", "--detach", tip)
        (root / "b.txt").write_text("tampered\n")
        sh("add", "."), sh("commit", "-q", "--amend", "--no-edit")
        tampered = sh("rev-parse", "HEAD^{tree}").stdout
        if tampered == rebuilt:
            bad.append("a tampered merge kept the rebuilt tree")
        # Main moved since the candidate's build: a plain push is refused, which is what `drop` reads.
        sh("checkout", "-q", "--detach", base)
        (root / "c.txt").write_text("moved\n")
        sh("add", "."), sh("commit", "-qm", "moved"), sh("push", "-q", "origin", "HEAD:main")
        if sh("push", "-q", "origin", f"{tip}:refs/heads/main").returncode == 0:
            bad.append("a candidate on an old base pushed over a moved main")
        # A conflict comes back as its path, and leaves no merge in progress.
        sh("checkout", "-q", "--detach", base)
        (root / "b.txt").write_text("other\n")
        sh("add", "."), sh("commit", "-qm", "other")
        if merge_member(root, 7, head, "A title", "topic") != ["b.txt"]:
            bad.append("a conflict did not name its path")
        if sh("rev-parse", "--verify", "--quiet", "MERGE_HEAD").returncode == 0:
            bad.append("a conflict left the merge in progress")
    return bad


def build_cases():
    """What build does around a record, with the forge and git replaced by a call list."""
    bad = []
    saved = {name: globals()[name] for name in ("git", "record_main", "use_driver", "merge_member")}
    pr = {"number": 7, "title": "t", "head": {"sha": "h", "ref": "b"}}
    for mode, recorded, want_record, want_build in (("on", True, True, True), ("on", False, True, False), ("shadow", False, False, True)):
        calls = []
        class Done:
            stdout = "abc\n"
        globals().update(git=lambda *a, **k: (calls.append(a[0:2]), Done)[1], use_driver=lambda cwd: None,
                         merge_member=lambda *a: [],
                         record_main=lambda recorded=recorded: (calls.append("record"), recorded)[1])
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                build({"queued": [pr]}, mode)
        finally:
            globals().update(saved)
        built = ("checkout", "-q") in calls
        if (calls[:1] == ["record"]) != want_record:
            bad.append(f"a build in {mode} read calls {calls[:2]}, wanting a record first: {want_record}")
        if built != want_build:
            bad.append(f"a record that returned {recorded} in {mode} read built={built}")
    return bad


def record_cases():
    """record_main's answers against real git: a stand-in recorder, a real push, real refusals."""
    bad = []
    recorder = "import pathlib\nwith pathlib.Path('docs/r.txt').open('a') as f:\n    f.write('rë\\n')\n"
    with tempfile.TemporaryDirectory(prefix="yi-record-main-") as tmp:
        tmp = pathlib.Path(tmp)
        origin, root, other = tmp / "origin.git", tmp / "w", tmp / "other"
        subprocess.run(("git", "init", "-q", "--bare", "-b", "main", str(origin)), check=True)
        for clone in (root, other):
            subprocess.run(("git", "clone", "-q", str(origin), str(clone)), capture_output=True, check=True)
        sh = lambda where, *a: git("-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *a, cwd=where, check=False)
        (root / "docs").mkdir()
        (root / "docs/r.txt").write_text("seed\n")
        sh(root, "add", "."), sh(root, "commit", "-qm", "seed"), sh(root, "push", "-q", "origin", "HEAD:main")
        (root / "scripts").mkdir()
        script = root / "scripts/record_changes.py"
        tip = lambda: git("rev-parse", "main", cwd=origin).stdout.strip()

        script.write_text("")
        was = tip()
        if not record_main(root) or tip() != was:
            bad.append("nothing to record did not answer True with main unmoved")

        script.write_text(recorder)
        if not record_main(root) or tip() == was:
            bad.append("a record did not reach main")
        if git("log", "-1", "--format=%an", tip(), cwd=origin).stdout.strip() != "yi-ci":
            bad.append("the record is not yi-ci's, so postmerge would not skip its own push")
        if git("rev-parse", "origin/main", cwd=root).stdout.strip() != tip():
            bad.append("origin/main lags the record, so the candidate would build on an old base")

        hook = origin / "hooks/pre-receive"
        hook.write_text("#!/bin/sh\necho protected >&2\nexit 1\n")
        hook.chmod(0o755)
        was, out = tip(), io.StringIO()
        with contextlib.redirect_stdout(out):
            kept = record_main(root)
        if not kept or tip() != was or "refused with main unmoved" not in out.getvalue() or "protected" not in out.getvalue():
            bad.append(f"a refusal with main unmoved answered {kept}: {out.getvalue()!r}")
        hook.unlink()

        # The race itself: another pusher wins between the recorder's commit and its push.
        sh(other, "pull", "-q", "origin", "main")
        (other / "moved.txt").write_text("moved\n")
        sh(other, "add", "."), sh(other, "commit", "-qm", "moved")
        race = root / ".git/hooks/pre-push"
        race.write_text(f"#!/bin/sh\nenv -u GIT_DIR git -C '{other}' push -q origin HEAD:main\n")
        race.chmod(0o755)
        if record_main(root) is not False:
            bad.append("main moved under the push and the tick read it as recorded")
        race.unlink()
    return bad


def shadow_cases():
    """Two ticks must not gate one PR's unchanged head twice: the note carries what the skip looks for."""
    head, saved, bad = "55a77022f2c6c470ae6bb5fc601c17236d8e883d", pr_review.comments, []
    note = shadow_note("e0f35fe52686c2d9a7b0d3f1c5e8a4b6d7f90123", head, "9d4f83c6f41f01b011ed48e063396ec46eceaa09")
    pr_review.comments = lambda repo, number: [{"body": note}]
    try:
        if not already_said(1812, head):
            bad.append(f"a head shadow announced reads unannounced: {note!r}")
        if already_said(1812, "0" * 40):
            bad.append("a moved head reads as already announced")
    finally:
        pr_review.comments = saved
    return bad


def selfcheck():
    bad = rule_cases() + git_cases() + build_cases() + record_cases() + shadow_cases()
    bad += [f"rule `{name}` disabled and every case still passes, so it refutes nothing"
            for name, _, _ in RULES if not rule_cases(name)]
    for gate in ("growth", "crate_size", "comments", "test_size"):
        run = subprocess.run((sys.executable, str(ROOT / f"scripts/guardrails/check_{gate}.py")), capture_output=True, text=True,
                             env={**os.environ, "GITHUB_REF": f"refs/heads/{CANDIDATE}"})
        if run.returncode or not run.stdout.startswith(f"skip {gate}"):
            bad.append(f"check_{gate} on a candidate read {run.returncode}: {run.stdout[:80]!r}")
    add = lambda who, at: {"type": "label", "label": {"name": LABEL}, "user": {"login": who}, "body": "1", "created_at": at}
    drop = {"type": "label", "label": {"name": LABEL}, "user": {"login": "jack"}, "body": ""}
    if enqueue_time([add("jack", "t1"), add("jack", "t2")]) != "t2":
        bad.append("an enqueue did not read its newest add")
    if enqueue_time([add("someone", "t1")]) is not None:
        bad.append("a label from outside MERGERS was honoured")
    if enqueue_time([add("jack", "t1"), drop]) is not None:
        bad.append("a removed label still read as queued")
    if closing("Closes #1\nrefs #2\ncloses #3\nCloses other/repo#4\nCloses #1") != [1, 3]:
        bad.append("closing() read the wrong issues")
    if bad:
        print("\n".join(f"FAIL queue selfcheck: {b}" for b in bad))
        return 1
    print("ok   queue selfcheck")
    return 0


def main(argv):
    if "--selfcheck" in argv:
        return selfcheck()
    mode = os.environ.get("MERGE_QUEUE", "off")
    if mode not in ("shadow", "on"):
        print(f"queue: MERGE_QUEUE is {mode}; nothing to do")
        return 0
    return tick(mode)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
