#!/usr/bin/env python3
"""The merge queue's tick: gate a PR merged onto current main, then push it and its record in one ref update.

    pr_queue.py               one tick, in the mode the MERGE_QUEUE variable names (shadow, on)
    pr_queue.py --selfcheck   every rule with each one disabled in turn, and the git steps for real

Nothing is stored: the state is the `queue` label, the `queue/candidate` ref and that ref's statuses,
so a crashed tick loses nothing. A red candidate stays until it is replaced, since it is the memory
of what comes next. Base and retry markers ride the tip's body. Recovery is deleting the ref (docs/FORGE.md)."""
import argparse, contextlib, io, os, pathlib, re, shutil, subprocess, sys, tempfile, time
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "scripts"), str(ROOT / "scripts/guardrails")]
import forge_pr, forge_tracking, pr_review
from check_pr_metadata import CITE
from _common import scratch_repo

LABEL, CANDIDATE = "queue", "queue/candidate"
# Sessions sign in as `jack` too, so an enqueue only when the owner asked stays a rule of conduct.
MERGERS = {"jack"}
K = 4
# Invariant: the candidate's three lanes run back to back on one runner, at most 3 x 20 minutes, and a run that
# has not reported by then is lost, because a candidate whose candidate.yml runs no lanes has no statuses at all.
STALE_MINUTES = 80
# Invariant: past the lanes' 80 minutes, and longer than the 100-minute `database is locked` fault of 2026-11-02.
CLOSE_PATIENCE = 240
INFRA_RETRIES = 2
IDENTITY = ("-c", "user.name=yi-queue", "-c", "user.email=queue@yi")
CI = ("-c", "user.name=yi-ci", "-c", "user.email=ci@yi")
LANES = pr_review.GATE_LANES
RETRY = re.compile(r"^Queue retry (\d+)\.$", re.M)
FORK = re.compile(r"^Queue base ([0-9a-f]{40})\.$", re.M)
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
    ("red batch", lambda o: o["candidate"] and len(o["members"]) > 1, "split"),
    ("red, first time", lambda o: o["candidate"] and o["retries"] == 0, "retry"),
    ("red, retried", lambda o: o["candidate"], "eject"),
    ("queue waiting", lambda o: o["queued"], "build"),
    ("nothing to do", lambda o: True, "idle"),
)


def step(obs, off=""):
    return next(((action, name) for name, holds, action in RULES if name != off and holds(obs)), ("none", "none"))


def git(*args, cwd=None, check=True):
    cwd = ROOT if cwd is None else cwd
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
    with pr_review.scratch_worktree(git, base, "yi-queue-") as tree:
        use_driver(tree)
        for number, head, title, branch in recipe:
            if merge_member(tree, number, head, title, branch):
                return False
        return git("rev-parse", "HEAD^{tree}", cwd=tree).stdout == git("rev-parse", f"{tip}^{{tree}}").stdout


def main_conflict(pr):
    with pr_review.scratch_worktree(git, "origin/main", "yi-queue-main-") as tree:
        use_driver(tree)
        return merge_member(tree, *recipe_of(pr), pr.get("body") or "")


def api(method, path, payload=None):
    return forge_pr.fgj_api(method, f"repos/{forge_pr.repo()}/{path}", payload)


def listing(batch, what):
    """Incident: `database is locked` came back as an object whose keys the tick iterated, and 21 ticks
    died on an AttributeError that never said what the forge answered. The next tick starts over."""
    if isinstance(batch, list):
        return batch
    raise SystemExit(f"queue: the forge did not list {what}: {batch.get('message') if isinstance(batch, dict) else batch}")


def label_ids():
    return {label["name"]: label["id"] for label in listing(api("GET", "labels?limit=100") or [], "the labels")}


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
    transport = lambda method, url, _: listing(forge_pr.fgj_api(method, url) or [], f"#{number}'s timeline")
    return enqueue_time(forge_tracking.paged(transport, f"repos/{forge_pr.repo()}/issues/{number}/timeline?x=1"))


def queued_pulls():
    return [pr for pr in listing(forge_pr.open_pulls(), "the open pull requests") if LABEL in {label["name"] for label in pr.get("labels") or []}]


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


def base_of(tip, body, in_main):
    """The main a candidate was built on: the queue's own marker, else a pre-batch tip's first parent; None when unreadable.

    Invariant: the queue appends its marker last, and a marker that is not an ancestor of the tip is refused."""
    marks = FORK.findall(body)
    if marks and git("merge-base", "--is-ancestor", marks[-1], tip, check=False).returncode == 0:
        return marks[-1]
    if in_main:
        return None if marks else git("rev-parse", f"{tip}^1").stdout.strip()
    return git("merge-base", tip, "origin/main").stdout.strip()


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
        body = git("log", "-1", "--format=%B", tip).stdout
        obs["base"] = base_of(tip, body, obs["in_main"])
        if obs["base"] is None:
            raise SystemExit(f"queue: {CANDIDATE} at {tip[:10]} is in main under a refused base marker, so its members are "
                             f"unreadable; take `{LABEL}` off and close their issues by hand, then delete the ref")
        obs["members"] = members_of(tip, obs["base"])
        obs["lanes"] = forge_pr.head_jobs(forge_pr.repo(), tip)
        date = git("log", "-1", "--format=%ct", tip).stdout.strip()
        obs["age_minutes"] = (time.time() - int(date)) / 60
        retries = RETRY.findall(body)
        obs["retries"] = int(retries[-1]) if retries else 0
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


SUBJECT = "Record the change files main landed"


def record_docs(tree):
    """Run `tree`'s recorder under `tree` and list the docs paths it wrote; None when it refused."""
    if subprocess.run((sys.executable, str(tree / "scripts/record_changes.py")), cwd=tree, check=False).returncode:
        return None
    return [line[3:] for line in git("status", "--porcelain", "docs", cwd=tree).stdout.splitlines()]


def commit_record(identity, author, root=ROOT):
    """Commit the recorder's writes under `docs` as `identity`, and return the commit's sha."""
    git("add", "docs", cwd=root)
    git(*identity, "commit", "-q", f"--author={author}", "-m", SUBJECT, cwd=root)
    return git("rev-parse", "HEAD", cwd=root).stdout.strip()


def record_main(root=ROOT):
    """Record what main has landed; False only when main moved under the push."""
    git("fetch", "-q", "origin", "main", cwd=root)
    git("checkout", "-q", "--detach", "origin/main", cwd=root)
    written = record_docs(root)
    if written is None:
        print("queue: the recorder refused, so main stays unrecorded; its lines above say why")
        return True
    if not written:
        return True
    commit_record(CI, "yi-ci <ci@yi>", root)
    pushed = git("push", "-q", "origin", "HEAD:main", cwd=root, check=False)
    if not pushed.returncode:
        return True
    git("fetch", "-q", "origin", "main", cwd=root)
    if git("rev-parse", "origin/main", cwd=root).stdout != git("rev-parse", "HEAD^", cwd=root).stdout:
        return False
    # Invariant: a candidate does not depend on the record, so a refusal that is not a moved main never stops the queue.
    print(f"queue: the recorder's push was refused with main unmoved: {pushed.stderr.strip()[:200]}")
    return True


def record_candidate(candidate):
    """The tip to push and whether its record is included or needs no write; False means no record.

    Invariant: main's recorder runs, never the candidate's, because this job holds the token that pushes
    main; the candidate supplies the docs it is recorded from and never the process that runs."""
    with pr_review.scratch_worktree(git, "origin/main", "yi-record-", name="main") as work:
        git("rm", "-rq", "docs", cwd=work)
        git("checkout", candidate, "--", "docs", cwd=work)
        git(*IDENTITY, "commit", "-q", "--allow-empty", "-m", "the candidate's docs", cwd=work)
        written = record_docs(work)
        kept = {path: (work / path).read_text() for path in written or [] if (work / path).is_file()}
    if written is None:
        print("queue: the recorder refused, so the candidate lands unrecorded; finish retries")
        return candidate, False
    git("checkout", "-q", "--detach", candidate, cwd=ROOT)
    for path, text in kept.items():
        (ROOT / path).parent.mkdir(parents=True, exist_ok=True)
        (ROOT / path).write_text(text)
    if not kept:
        return candidate, True
    # Invariant: the push's head stays yi-queue's, so postmerge skips the record it already contains.
    return commit_record(IDENTITY, "yi-queue <queue@yi>", ROOT), True


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
            if built:
                left = main_conflict(pr)
                if not left:
                    continue
            names = ", ".join(f"`{path}`" for path in left)
            comment(pr["number"], f"The queue could not merge this onto main: {names}. The fixer takes it from here, and the label stays.")
            set_label(pr["number"], "autofix", True)
            continue
        built.append(pr["number"])
    if not built:
        print("queue: nothing merged, so no candidate")
        return
    tip = git("rev-parse", "HEAD").stdout.strip()
    base = git("rev-parse", "origin/main").stdout.strip()
    body = git("log", "-1", "--format=%B", "HEAD").stdout.strip()
    tip = rewrite(tip, f"{body}\n\nQueue base {base}.")
    git("push", "-q", "--force", "origin", f"{tip}:refs/heads/{CANDIDATE}")
    print(f"queue: candidate {tip[:10]} for {built}")


def rewrite(tip, message):
    parents = git("log", "-1", "--format=%P", tip).stdout.split()
    env = {**os.environ, "GIT_AUTHOR_NAME": "yi-queue", "GIT_AUTHOR_EMAIL": "queue@yi",
           "GIT_COMMITTER_NAME": "yi-queue", "GIT_COMMITTER_EMAIL": "queue@yi"}
    args = ["git", "-C", str(ROOT), "commit-tree", f"{tip}^{{tree}}", "-m", message]
    for parent in parents:
        args += ["-p", parent]
    return subprocess.run(args, capture_output=True, text=True, env=env, check=True).stdout.strip()


def retry(obs):
    body = RETRY.sub("", git("log", "-1", "--format=%B", obs["candidate"]).stdout).strip()
    made = rewrite(obs["candidate"], f"{body}\n\nQueue retry {obs['retries'] + 1}.")
    git("push", "-q", "--force", "origin", f"{made}:refs/heads/{CANDIDATE}")
    print(f"queue: retry {obs['retries'] + 1} as {made[:10]}")


def split(obs, mode):
    half = (len(obs["members"]) + 1) // 2
    by_number = {pr["number"]: pr for pr in obs["pulls"]}
    queued = [by_number[number] for number, _ in obs["members"][:half]]
    # ponytail: the right half may rebundle with newer PRs; keeping it intact requires a store.
    build({**obs, "queued": queued}, mode)


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


def finish(obs, recorded=False):
    """Idempotent, so a tick killed after the push does this through the `landed` rule instead."""
    for number, _ in obs["members"]:
        pr = forge_pr.pull(number)
        # The forge marks the PR but fires no closing keyword on a push (the 2026-11-01 probe).
        for issue in closing(pr.get("body")):
            answer = api("PATCH", f"issues/{issue}", {"state": "closed"})
            refused = "the forge answered 404" if answer is None else answer.get("message") if isinstance(answer, dict) else None
            if refused and obs.get("age_minutes", 0) < CLOSE_PATIENCE:
                raise SystemExit(f"queue: the forge refused to close #{issue}, and the next tick tries again: {refused}")
            if refused:
                print(f"queue: gave up closing #{issue}: {refused}")
                comment(number, f"Landed, but the forge refused to close #{issue}: {refused}. It needs closing by hand.")
        set_label(number, LABEL, False)
    if not recorded and not record_main():
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
    tip, recorded = record_candidate(obs["candidate"])
    pushed = git("push", "-q", "origin", f"{tip}:refs/heads/main", check=False)
    if pushed.returncode:
        print(f"queue: main refused the push, so the candidate goes: {pushed.stderr.strip()[:200]}")
        return delete_ref()
    finish(obs, recorded=recorded)


def tick(mode):
    try:
        obs = observe(mode)
    except pr_review.Unanswered as err:
        raise SystemExit(f"queue: {err}") from err
    action, why = step(obs)
    print(f"queue: {action} ({why}){' ' + obs['stale_member'] if obs['stale_member'] else ''}")
    if action == "drop":
        delete_ref()
    elif action == "build":
        build(obs, mode)
    elif action == "retry":
        retry(obs)
    elif action == "split":
        split(obs, mode)
    elif action == "eject":
        eject(obs)
    elif action == "land":
        land(obs, mode)
    elif action == "finish":
        # Invariant: shadow writes neither main nor issues, and the ref is how the next `on` tick knows to finish.
        if mode == "on":
            finish(obs)
    elif action == "paused":
        print(f"queue: paused on {obs['candidate'][:10]}, whose lanes read {obs['lanes']}; deleting {CANDIDATE} resumes")
    return 0


def cmd_queue(args, mode=None):
    """Add one PR, refusing what a tick would skip; with no number, print the queue."""
    mode = forge_pr.queue_mode() if mode is None else mode
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
    if mode == "off":
        # Invariant: the workflow's own `if` skips every tick under off, so the label would land nothing.
        print(f"queue: MERGE_QUEUE is off, so no tick reads the label; #{number} is not queued")
        return 1
    set_label(number, LABEL, True)
    print(f"#{number} queued; " + ("a tick lands it" if mode == "on" else "a shadow tick says what it would land, and nothing lands"))
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
       "age_minutes": 5, "retries": 0, "members": [(1, "h")], "queued": [], "pulls": []}
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
    ("infra, retries left", dict(lanes={lane: "failure" for lane in LANES}, members=[(1, "h"), (2, "i")]), "retry"),
    ("infra, retries spent", dict(lanes={lane: "failure" for lane in LANES}, retries=INFRA_RETRIES), "paused"),
    ("red batch", dict(lanes=RED, members=[(1, "h"), (2, "i")]), "split"),
    ("red batch", dict(lanes=RED, retries=1, members=[(1, "h"), (2, "i")]), "split"),
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
        stranger = sh("rev-parse", "HEAD").stdout.strip()
        with mock.patch.object(sys.modules[__name__], "ROOT", root):
            for body in (f"Queue base {stranger}.\n", f"Queue base {stranger}.\n\nQueue base {base}.\n"):
                if (got := base_of(tip, body, False)) != base:
                    bad.append(f"a candidate whose body reads {body!r} took base {got[:10]}, not its fork {base[:10]}")
            if (got := base_of(tip, f"Queue base {stranger}.\n", True)) is not None:
                bad.append(f"a landed candidate with a refused base marker took base {got[:10]}, so it would half-finish")
    return bad


@contextlib.contextmanager
def scenario_repo(numbers, shared=False):
    with tempfile.TemporaryDirectory(prefix="yi-queue-scenarios-") as tmp:
        tmp = pathlib.Path(tmp)
        root, origin = tmp / "w", tmp / "origin.git"
        subprocess.run(("git", "init", "-q", "--bare", "-b", "main", str(origin)), check=True)
        subprocess.run(("git", "clone", "-q", str(origin), str(root)), capture_output=True, check=True)
        real_git = git
        sh = lambda *a: real_git("-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *a, cwd=root, check=False)
        (root / "seed.txt").write_text("seed\n")
        if shared:
            (root / "shared.txt").write_text("base\n")
        sh("add", "."), sh("commit", "-qm", "seed"), sh("push", "-q", "origin", "HEAD:main"), sh("fetch", "-q", "origin", "main")
        base = sh("rev-parse", "HEAD").stdout.strip()
        pulls = {}
        for number in numbers:
            sh("checkout", "-q", "--detach", base)
            path = root / ("shared.txt" if shared else f"pr-{number}.txt")
            path.write_text(f"pr {number}\n")
            sh("add", "."), sh("commit", "-qm", f"pr {number}")
            head = sh("rev-parse", "HEAD").stdout.strip()
            sh("push", "-q", "origin", f"{head}:refs/pull/{number}/head")
            pulls[number] = {"number": number, "title": f"PR {number}", "body": "", "head": {"sha": head, "ref": f"topic-{number}"}}
        queued, comments, autofix, pushes = set(numbers), [], set(), []

        def counted(*args, cwd=None, check=True):
            if args[:1] == ("push",) and any(arg.endswith(":refs/heads/main") for arg in args):
                pushes.append(args)
            return real_git(*args, cwd=cwd, check=check)

        def label(number, name, present):
            if name == LABEL and not present:
                queued.discard(number)
            if name == "autofix" and present:
                autofix.add(number)

        def remove():
            state["landed_obs"] = observe("on")
            real_git("push", "-q", "origin", "--delete", CANDIDATE, cwd=root, check=False)
        def jobs(repo, tip):
            red = state.get("red")
            return RED if red and red in [number for number, _ in members_of(tip, state["base"])] else GREEN

        module = sys.modules[__name__]
        with mock.patch.multiple(module, ROOT=root, git=counted, record_main=lambda root=ROOT: True,
                                 queued_pulls=lambda: [pulls[number] for number in sorted(queued)], enqueued_at=lambda number: number,
                                 dequeue_reason=lambda pr: None, eligible_problem=lambda pr, jobs: None,
                                 record_candidate=lambda candidate: (candidate, True), use_driver=lambda cwd: None,
                                 comment=lambda number, text: comments.append((number, text)), set_label=label, delete_ref=remove,
                                 statuses_url=lambda sha, lane: ""), \
                mock.patch.multiple(forge_pr, pull=lambda number: pulls[number], is_behind=lambda pr: False, repo=lambda: "r",
                                    head_jobs=jobs, unreviewed=lambda number, pr: []):
            state = {"base": base, "pulls": pulls, "queued": queued, "comments": comments, "autofix": autofix, "pushes": pushes}
            yield state

def drive_scenario(state, red=None, off=""):
    actions, bad = [], []
    state["red"] = red
    real_step = step
    for _ in range(20):
        seen = []
        pushes = len(state["pushes"])
        with mock.patch.object(sys.modules[__name__], "step", lambda obs: (seen.append(obs), real_step(obs, off))[1]), \
                contextlib.redirect_stdout(io.StringIO()):
            tick("on")
        action, _ = real_step(seen[-1], off)
        actions.append(action)
        if action == "idle":
            return actions, bad
        before = [number for number, _ in seen[-1]["members"]]
        if action == "land" and len(state["pushes"]) != pushes + 1:
            bad.append(f"a green candidate made {len(state['pushes']) - pushes} main pushes, not one")
        if action == "split":
            want = before[:(len(before) + 1) // 2]
            got = [number for number, _ in observe("on")["members"]]
            if got != want:
                bad.append(f"a split of {before} built {got}, not {want}")
    return actions, bad + [f"scenario did not become idle: {actions}"]


def batch_cases(off=""):
    bad = []
    with scenario_repo((1, 2, 3, 4)) as state:
        actions, found = drive_scenario(state, red=4, off=off)
        bad += found
        want = ["build", "split", "land", "build", "split", "land", "build", "retry", "eject", "idle"]
        if actions != want:
            bad.append(f"batch of four with one planted red PR read actions {actions}, not {want}")
        main = git("rev-parse", "origin/main").stdout.strip()
        landed = [number for number, _ in members_of(main, state["base"])]
        if landed != [1, 2, 3]:
            bad.append(f"the red batch left {landed} on main, not [1, 2, 3]")
        ejected = {number for number, text in state["comments"] if "Left the queue" in text}
        if ejected != {4}:
            bad.append(f"the red batch ejected {ejected}, not {{4}}")
    return bad


def conflict_cases(off=""):
    bad = []
    with scenario_repo((1, 2), shared=True) as state:
        module = sys.modules[__name__]
        disabled = mock.patch.object(module, "main_conflict", lambda pr: ["shared.txt"]) if off else contextlib.nullcontext()
        with disabled, contextlib.redirect_stdout(io.StringIO()):
            build(observe("on"), "on")
        obs = observe("on")
        members = [number for number, _ in obs["members"]]
        if members != [1]:
            bad.append(f"a member-only conflict left candidate members {members}, not [1]")
        if state["comments"] or state["autofix"]:
            bad.append(f"member-only conflict deferral wrote comments={state['comments']} or autofix={state['autofix']}")
        if 2 not in state["queued"]:
            bad.append("a member-only conflict removed the deferred PR from the queue")
    return bad


def green_cases(off=""):
    bad = []
    with scenario_repo((1, 2, 3, 4)) as state:
        module = sys.modules[__name__]
        disabled = mock.patch.object(module, "FORK", re.compile(r"a^")) if off == "FORK" else (
            mock.patch.object(module, "K", 1) if off else contextlib.nullcontext())
        with disabled:
            actions, found = drive_scenario(state)
        bad += found
        if actions != ["build", "land", "idle"]:
            bad.append(f"green batch of four read actions {actions}")
        main = git("rev-parse", "origin/main").stdout.strip()
        landed = [number for number, _ in members_of(main, state["base"])]
        if landed != [1, 2, 3, 4]:
            bad.append(f"green batch of four landed {landed}")
        if len(state["pushes"]) != 1 or any("--force" in call for call in state["pushes"]):
            bad.append(f"green batch of four needed one main push without force, got {state['pushes']}")
        if state["queued"]:
            bad.append(f"the landed path did not finish every member: {state['queued']}")
        recovered = state.get("landed_obs", {})
        if not recovered.get("in_main") or [number for number, _ in recovered.get("members", [])] != [1, 2, 3, 4]:
            bad.append(f"a landed batch lost its members before recovery: {recovered}")
    return bad


def candidate_gate_cases():
    bad = []
    seed = {"crates/a/src/lib.rs": "// one\nfn a() {}\n", "crates/a/tests/t.rs": "fn t() {}\n"}
    with scratch_repo("yi-queue-gates-", seed) as (tmp, run):
        with open(tmp / "crates/a/src/lib.rs", "a") as f:
            f.write("// two\n// three\n// four\n" + "".join(f"fn g{i}() {{}}\n" for i in range(200)))
        (tmp / "crates/a/tests/t.rs").write_text("fn t() {}\nfn u() {}\n")
        for gate in ("growth", "crate_size", "comments", "test_size"):
            script = f"scripts/guardrails/check_{gate}.py"
            disabled = run(sys.executable, script)
            if disabled.returncode == 0:
                bad.append(f"check_{gate} with its candidate skip disabled passed: {disabled.stdout!r}")
            candidate = subprocess.run((sys.executable, script), cwd=tmp, capture_output=True, text=True,
                                       env={**os.environ, "GITHUB_REF": f"refs/heads/{CANDIDATE}"})
            want = f"skip {gate} (queue candidate: each member was judged at its own head)"
            if candidate.returncode or candidate.stdout.strip() != want:
                bad.append(f"check_{gate} on a candidate read {candidate.returncode}: {candidate.stdout.strip()!r}, not {want!r}")
    return bad


def build_cases():
    """What build does around a record, with the forge and git replaced by a call list."""
    bad = []
    saved = {name: globals()[name] for name in ("git", "record_main", "use_driver", "merge_member", "rewrite")}
    pr = {"number": 7, "title": "t", "head": {"sha": "h", "ref": "b"}}
    for mode, recorded, want_record, want_build in (("on", True, True, True), ("on", False, True, False), ("shadow", False, False, True)):
        calls = []
        class Done:
            stdout = "abc\n"
        globals().update(git=lambda *a, **k: (calls.append(a[0:2]), Done)[1], use_driver=lambda cwd: None,
                         merge_member=lambda *a: [], rewrite=lambda tip, message: "abc",
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


@contextlib.contextmanager
def temp_origin(prefix):
    """A bare origin, one clone of it with the seeded docs/ and stand-in recorder, and a clone(name) for another."""
    with tempfile.TemporaryDirectory(prefix=prefix) as tmp:
        tmp = pathlib.Path(tmp)
        origin = tmp / "origin.git"
        subprocess.run(("git", "init", "-q", "--bare", "-b", "main", str(origin)), check=True)

        def clone(name):
            subprocess.run(("git", "clone", "-q", str(origin), str(tmp / name)), capture_output=True, check=True)
            return tmp / name

        root = clone("w")

        def sh(*a, where=root):
            return git("-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *a, cwd=where, check=False)

        (root / "docs").mkdir()
        (root / "scripts").mkdir()
        (root / "docs/r.txt").write_text("seed\n")
        (root / "scripts/record_changes.py").write_text("")
        sh("add", "."), sh("commit", "-qm", "seed"), sh("push", "-q", "origin", "HEAD:main")
        sh("fetch", "-q", "origin", "main")
        yield tmp, origin, root, sh, clone


RECORDER = "import pathlib\nwith pathlib.Path('docs/r.txt').open('a') as f:\n    f.write('rë\\n')\n"


def record_cases():
    """record_main's answers against real git: a stand-in recorder, a real push, real refusals."""
    bad = []
    with temp_origin("yi-record-main-") as (tmp, origin, root, sh, clone):
        other = clone("other")
        script = root / "scripts/record_changes.py"
        tip = lambda: git("rev-parse", "main", cwd=origin).stdout.strip()

        was = tip()
        if not record_main(root) or tip() != was:
            bad.append("nothing to record did not answer True with main unmoved")

        script.write_text(RECORDER)
        with mock.patch.dict(os.environ, {"GIT_AUTHOR_NAME": "hook", "GIT_AUTHOR_EMAIL": "hook@hook"}):
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

        script.write_text("import sys\nprint('record: refused, nothing written')\nsys.exit(1)\n")
        was, out = tip(), io.StringIO()
        with contextlib.redirect_stdout(out):
            kept = record_main(root)
        if not kept or tip() != was or "recorder refused" not in out.getvalue():
            bad.append(f"a recorder's refusal stopped the tick: {kept}, {out.getvalue()!r}")
        script.write_text(RECORDER)

        # The race itself: another pusher wins between the recorder's commit and its push.
        sh("pull", "-q", "origin", "main", where=other)
        (other / "moved.txt").write_text("moved\n")
        sh("add", ".", where=other), sh("commit", "-qm", "moved", where=other)
        race = root / ".git/hooks/pre-push"
        race.write_text(f"#!/bin/sh\nenv -u GIT_DIR git -C '{other}' push -q origin HEAD:main\n")
        race.chmod(0o755)
        real_git = git
        def race_git(*args, **kwargs):
            return real_git("-c", "core.hooksPath=.git/hooks", *args, **kwargs)
        with mock.patch.object(sys.modules[__name__], "git", race_git):
            moved = record_main(root)
        if moved is not False:
            bad.append("main moved under the push and the tick read it as recorded")
        race.unlink()
    return bad

def landing_cases():
    """A candidate and its record reach main through one push, and the record is main's over the candidate's docs."""
    bad = []
    with temp_origin("yi-land-") as (tmp, origin, root, sh, clone):
        (root / "scripts/record_changes.py").write_text(RECORDER)
        sh("add", "."), sh("commit", "-qm", "recorder"), sh("push", "-q", "origin", "HEAD:main"), sh("fetch", "-q", "origin", "main")
        base = sh("rev-parse", "HEAD").stdout.strip()
        (root / "scripts/record_changes.py").write_text("import pathlib\npathlib.Path('docs/from-the-pr.txt').write_text('x')\n")
        (root / "candidate.txt").write_text("candidate\n")
        sh("add", "."), sh("commit", "-qm", "candidate")
        candidate = sh("rev-parse", "HEAD").stdout.strip()
        pushes = tmp / "pushes"
        hook = origin / "hooks/pre-receive"
        hook.write_text(f"#!/bin/sh\necho push >> '{pushes}'\n")
        hook.chmod(0o755)
        obs = {"base": base, "candidate": candidate, "members": [(7, candidate)]}
        pr = {"title": "A title", "head": {"ref": "topic"}, "body": ""}
        real_git, real_record_main = git, record_main
        at_root = lambda *a, **kw: real_git(*a, **{"cwd": root, **kw})
        with mock.patch.object(forge_pr, "pull", lambda number: pr), \
                mock.patch.multiple(sys.modules[__name__], same_tree=lambda *a: True, git=at_root, ROOT=root,
                                    record_main=lambda: real_record_main(root), set_label=mock.DEFAULT, delete_ref=mock.DEFAULT):
            land(obs, "on")
        landed = git("rev-parse", "main", cwd=origin).stdout.strip()
        count = len(pushes.read_text().splitlines()) if pushes.exists() else 0
        if count != 1:
            bad.append(f"a landing and its record took {count} pushes, not one")
        if git("rev-parse", f"{landed}^", cwd=origin).stdout.strip() != candidate:
            bad.append("main's record commit is not on the gated candidate")
        if git("log", "-1", "--format=%an", landed, cwd=origin).stdout.strip() != "yi-queue":
            bad.append("a recorded landing's head is not yi-queue's, so postmerge would record it again")
        if git("show", "main:docs/r.txt", cwd=origin).stdout != "seed\nrë\n":
            bad.append("the landing did not include the recorder's write")
        if git("show", "main:docs/from-the-pr.txt", cwd=origin, check=False).returncode == 0:
            bad.append("the candidate's own recorder ran inside the job that pushes main")
    return bad

def shadow_cases():
    """Shadow's real paths with the forge stubbed: a head is announced once, and nothing lands or records."""
    bad, posted = [], []
    heads = {1812: "55a77022f2c6c470ae6bb5fc601c17236d8e883d", 1857: "71c203e2372f576ae772b9a4cedc59a2fd7e907d"}
    obs = {"candidate": "e0f35fe52686c2d9a7b0d3f1c5e8a4b6d7f90123", "base": "9d4f83c6f41f01b011ed48e063396ec46eceaa09",
           "members": list(heads.items()), "stale_member": None}
    pull = lambda number: {"title": "t", "head": {"ref": "b"}}
    with mock.patch.object(forge_pr, "pull", pull), mock.patch.object(pr_review, "comments", lambda repo, n: [{"body": b} for m, b in posted if m == n]), \
            mock.patch.multiple(sys.modules[__name__], same_tree=lambda *a: True, comment=lambda n, b: posted.append((n, b)),
                                delete_ref=mock.DEFAULT, finish=mock.DEFAULT, observe=lambda mode: obs, step=lambda o: ("finish", "landed")) as stubs:
        land(obs, "shadow")
        for number, head in heads.items():
            if not already_said(number, head):
                bad.append(f"#{number}'s head reads unannounced after shadow's comment: {posted}")
        if already_said(1812, heads[1857]) or already_said(1812, "0" * 40):
            bad.append(f"a head #1812 never had reads announced: {posted}")
        with contextlib.redirect_stdout(io.StringIO()):
            tick("shadow")
            if stubs["finish"].called:
                bad.append("a shadow tick over a landed candidate ran finish, which writes main")
            if stubs["delete_ref"].call_count != 1:
                bad.append("a shadow tick over a landed candidate deleted the ref an `on` tick finishes from")
            tick("on")
            if not stubs["finish"].called:
                bad.append("an `on` tick over a landed candidate did not finish")
    return bad


def forge_cases():
    """A forge that answers with an error object, as `database is locked` did for 100 minutes on 2026-11-02."""
    bad = []
    locked = {"message": "database is locked", "url": "https://git.example.invalid/api/swagger"}
    calls = {"queued_pulls": queued_pulls, "enqueued_at": lambda: enqueued_at(1), "label_ids": label_ids,
             "tick": lambda: tick("on")}
    with mock.patch.object(forge_pr, "repo", lambda: "o/r"), mock.patch.object(sys.modules[__name__], "git", mock.Mock()):
        # A body that is JSON but not an object still has to reach the log.
        with mock.patch.object(forge_pr, "fgj_api", lambda *a: "bad gateway"):
            try:
                queued_pulls()
                bad.append("a string answer read as a listing")
            except SystemExit as stop:
                if "bad gateway" not in str(stop):
                    bad.append(f"a string answer stopped without its words: {stop}")
            except Exception as err:
                bad.append(f"a string answer died: {err!r}")
        for name, call in calls.items():
            with mock.patch.object(forge_pr, "fgj_api", lambda *a: locked):
                try:
                    call()
                    bad.append(f"{name} took an error object for an answer")
                except SystemExit as stop:
                    if "database is locked" not in str(stop):
                        bad.append(f"{name} stopped without the forge's words: {stop}")
                except Exception as err:
                    bad.append(f"{name} died on an error object: {err!r}")
        # A round reader's miss is the review job's Unanswered, which a tick turns into the same stop.
        queued = {"number": 1, "labels": [{"name": LABEL}]}
        answers = lambda method, path, payload=None: locked if path.endswith("/comments") else [queued] if "page=1" in path else []
        with mock.patch.object(forge_pr, "fgj_api", answers):
            try:
                tick("on")
                bad.append("a tick took an error object for the comments")
            except SystemExit as stop:
                if "database is locked" not in str(stop):
                    bad.append(f"a tick stopped on the comments without the forge's words: {stop}")
            except Exception as err:
                bad.append(f"a tick died on the comments: {err!r}")
            try:
                pr_review.comments("o/r", 1)
                bad.append("comments took an error object for an answer")
            except pr_review.Unanswered as err:
                if "database is locked" not in str(err):
                    bad.append(f"comments' miss lost the forge's words: {err}")
            except BaseException as err:
                bad.append(f"comments stopped outside the handlers that retry a miss: {err!r}")
        # A refused close holds the ref inside the patience, so the `landed` rule retries it, and never wedges past it.
        forge = lambda method, path, payload=None: locked if method == "PATCH" else {"body": "Closes #5"}
        for age, keeps in ((CLOSE_PATIENCE - 1, True), (CLOSE_PATIENCE, False)):
            obs = {"members": [(1, "a" * 40)], "age_minutes": age}
            with mock.patch.object(forge_pr, "fgj_api", forge), mock.patch.object(forge_pr, "pull", lambda n: {"body": "Closes #5"}), \
                    mock.patch.multiple(sys.modules[__name__], delete_ref=mock.DEFAULT, record_main=mock.DEFAULT, set_label=mock.DEFAULT, comment=mock.DEFAULT) as stubs:
                try:
                    with contextlib.redirect_stdout(io.StringIO()):
                        finish(obs)
                    stopped = False
                except SystemExit:
                    stopped = True
                if stopped != keeps or stubs["delete_ref"].called == keeps:
                    bad.append(f"a refused close at {age} minutes stopped={stopped}, deleted the ref={stubs['delete_ref'].called}")
                if not keeps and "#5" not in str(stubs["comment"].call_args):
                    bad.append("a close given up on did not say so on the PR")
        # A 404 may be a gone issue or a write the token may not make, so only the closed issue itself is an answer.
        for answer, closed in (({"number": 5, "state": "closed"}, True), (None, False)):
            with mock.patch.object(forge_pr, "fgj_api", lambda *a: answer), mock.patch.object(forge_pr, "pull", lambda n: {"body": "Closes #5"}), \
                    mock.patch.multiple(sys.modules[__name__], delete_ref=mock.DEFAULT, record_main=mock.DEFAULT, set_label=mock.DEFAULT, comment=mock.DEFAULT) as stubs:
                try:
                    with contextlib.redirect_stdout(io.StringIO()):
                        finish({"members": [(1, "a" * 40)], "age_minutes": 1})
                except SystemExit:
                    pass
                if stubs["comment"].called or stubs["delete_ref"].called != closed:
                    bad.append(f"a close answered {answer} deleted the ref={stubs['delete_ref'].called}")
    return bad


def queue_verb_cases():
    """cmd_queue refuses a queue no tick reads, and labels a PR only where a tick will land it."""
    bad, labels = [], []
    pr = {"number": 7, "title": "t", "head": {"sha": "h", "ref": "b"}, "base": {"ref": "main"}}
    with mock.patch.object(forge_pr, "pull_number", lambda number: 7), \
            mock.patch.object(forge_pr, "pull", lambda number: pr), \
            mock.patch.object(forge_pr, "jobs_of", lambda number: []), \
            mock.patch.multiple(sys.modules[__name__], eligible_problem=lambda p, j: None,
                                set_label=lambda number, name, present: labels.append((number, name, present))):
        with contextlib.redirect_stdout(io.StringIO()) as told:
            refused = cmd_queue(argparse.Namespace(number=7), "off")
        off_label = list(labels)
        with contextlib.redirect_stdout(io.StringIO()):
            queued = cmd_queue(argparse.Namespace(number=7), "on")
    if refused != 1 or off_label or "off" not in told.getvalue():
        bad.append(f"an off queue answered {refused}, labelled {off_label}, said {told.getvalue()!r}")
    if queued != 0 or labels != [(7, LABEL, True)]:
        bad.append(f"an on queue answered {queued} and labelled {labels}")
    return bad


def offline_cases():
    """Every case with the forge's read failing: one that reaches it is a gate `just check` cannot run, since
    the guardrails lane installs no fgj."""
    cases = (rule_cases, git_cases, build_cases, record_cases, landing_cases, shadow_cases, forge_cases, queue_verb_cases,
             batch_cases, conflict_cases, green_cases, candidate_gate_cases)
    bad = []
    with mock.patch.object(forge_pr, "fgj_api", side_effect=RuntimeError("fgj")):
        for case in cases:
            try:
                bad += case()
            except RuntimeError as refused:
                if str(refused) != "fgj":
                    raise
                bad.append(f"{case.__name__} asked the forge, which `just check` cannot answer")
    return bad


def selfcheck():
    bad = offline_cases()
    bad += [f"rule `{name}` disabled and every case still passes, so it refutes nothing"
            for name, _, _ in RULES if not rule_cases(name)]
    for name, call in (("red batch", lambda: batch_cases("red batch")), ("member-only conflict", lambda: conflict_cases("main_conflict")),
                       ("green batch of four", lambda: green_cases("K")), ("landed batch recovery", lambda: green_cases("FORK"))):
        if not call():
            bad.append(f"{name} passes with its new behavior disabled, so its scenario refutes nothing")
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
