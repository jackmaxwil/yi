#!/usr/bin/env python3
"""The merge queue's tick: gate a PR merged onto current main, then push it and its record in one ref update.

    pr_queue.py               one tick, in the mode the MERGE_QUEUE variable names (shadow, on)
    pr_queue.py --selfcheck   every rule with each one disabled in turn, and the git steps for real

Nothing is stored: the state is the `queue` label, the `queue/candidate` ref and that ref's statuses,
so a crashed tick loses nothing. A red candidate stays until it is replaced, since it is the memory
of what comes next. Base and retry markers ride the tip's body. Recovery is deleting the ref (docs/FORGE.md)."""
import argparse, contextlib, datetime, functools, io, json, os, pathlib, re, shutil, subprocess, sys, tempfile, time, urllib.parse
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "scripts"), str(ROOT / "scripts/guardrails")]
import bot_meter, forge_pr, forge_tracking, pr_review
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


STATE, WOULD, FORGE_SECS = "yi/state", "yi/would", 60
BLOCKED = lambda f: f["verdict"] == "blocked"
CLEAN = lambda f: f["verdict"] in ("clean", "override") and f["gate"] == "green"


def held(f):
    """Incident: #2343, clean, green and queued, read `held` because the hold row came first, and the hold
    only keeps the fixer out; so it names the state only where the fixer would otherwise own the PR."""
    return "autofix:hold" in f["labels"] and next(row[1] for row in STATES if row[3] is not held and row[3](f)) == "fixer"


# Invariant: a PR's state is the first row that holds, and a clock is its owner's measured p90 latency on the
# 2026-10-20..11-24 forge record, or the code's own window where that owner is a timer. The last column is
# the act the tick owes the state under QUEUE_ROWS; a row without one is someone else's to move.
STATES = (
    ("held", "person", 22, held, None),
    ("fixer working", "fixer", 1, lambda f: "autofix:working" in f["labels"], None),
    ("fixer failed", "fixer", 12, lambda f: "autofix:failed" in f["labels"], None),
    ("fixer asked", "fixer", 12, lambda f: "autofix" in f["labels"], None),
    ("fixer declined", "person", 22, lambda f: f["fix"] == "declined", None),
    ("fixer handed off", "person", 22, lambda f: f["fix"] == "handoff", None),
    ("based off a closed branch", "queue", 0.25, lambda f: f["base"] != "main" and f.get("parent") == "closed", "repoint"),
    ("based off main", "queue", 10, lambda f: f["base"] != "main", None),
    ("awaiting review", "review", 18, lambda f: f["verdict"] is None and not f["lost"], None),
    ("lost answers", "review", 1.2, lambda f: f["lost"] and not f["retries_spent"], None),
    ("lost answers, retries spent", "person", 22, lambda f: f["lost"], None),
    ("blocked round, queued", "queue", 0.25, lambda f: BLOCKED(f) and "queue" in f["labels"], None),
    ("blocked round, draft", "fixer", 12, lambda f: BLOCKED(f) and f["draft"], None),
    ("blocked round, ready", "fixer", 12, BLOCKED, None),
    ("conflicted", "fixer", 12, lambda f: f["mergeable"] is False, None),
    ("title red", "author", 22, lambda f: f["gate"] == "title", None),
    ("gate red", "fixer", 4, lambda f: f["gate"] == "red", None),
    ("gate pending", "gate", STALE_MINUTES / 60, lambda f: f["gate"] == "pending", None),
    ("queued", "queue", 1, lambda f: "queue" in f["labels"] and CLEAN(f), None),
    ("clean draft", "review", 8, lambda f: f["draft"] and CLEAN(f), "promote"),
    ("clean, not queued", "person", 13, lambda f: not f["draft"] and CLEAN(f), "enqueue"),
    ("unknown", "person", 0, lambda f: True, "say"),
)


def state_of(f, off=""):
    """(state, owner, since, clock hours) of one PR's facts; only the rows' acts read it."""
    name, owner, clock, *_ = next((row for row in STATES if row[0] != off and row[3](f)), ("none", "person", 0, None, None))
    return name, owner, f["since"], clock


def facts(pr, notes, statuses, holds, who, parent=None):
    """What `state_of` reads, from the listing's PR, its comments, its head's status rows and the round authors."""
    head = pr["head"]["sha"]
    seen = [a for a in pr_review.attempts_of(notes, who, pr["number"]) if holds(a["sha"], head)]
    need, jobs = (*LANES, "title"), forge_pr.as_jobs(statuses)
    times = [pr_review.when(pr["updated_at"]), *(pr_review.when(s.get("updated_at") or pr["updated_at"])
                                                 for s, (job, _, _) in zip(statuses, jobs) if job in need)]
    verdict = forge_pr.decide({"state": "open"}, forge_pr.job_states(jobs), need, False)
    gate = "title" if verdict == "failed:title" else "red" if verdict.startswith("failed:") else verdict
    fixes = [r for r in bot_meter.rows(notes) if r["kind"] == "yi-autofix" and r.get("from") == head[:12]]
    rounds = [a for a in seen if a.get("kind") == "round"]
    return {"labels": {label["name"] for label in pr.get("labels") or []}, "draft": pr["title"].startswith(forge_pr.DRAFT),
            "base": pr["base"]["ref"], "parent": parent, "gate": gate, "mergeable": pr.get("mergeable"), "since": max(times),
            "fix": fixes[-1].get("verdict") if fixes else None,
            "lost": bool(seen) and pr_review.lost_attempt(seen[-1]), "retries_spent": len(seen) > pr_review.RETRIES,
            "verdict": rounds[-1]["verdict"] if rounds and not pr_review.lost_attempt(seen[-1]) else None}


def describe(state, owner, since, clock, now):
    age = (now - since) / 3600
    when = datetime.datetime.fromtimestamp(since, datetime.timezone.utc).strftime("%m-%d %H:%M")
    return f"{state}, waiting on {owner} since {when}Z ({age:.0f} h, clock {clock:.3g} h)" + (" parked" if age > clock else "")


def said(description):
    """The part of a description whose change is worth a write: state, owner, and the clock crossed."""
    return description.split(" since ")[0], description.endswith(" parked")


def assess(pr, who, notes, parent=None):
    """(state, owner, since, clock, the head's status rows, the facts, the round holds) as the forge has them now."""
    sha, base = pr["head"]["sha"], pr["base"]["ref"]
    rows = forge_pr.status_rows(forge_pr.repo(), sha, timeout=FORGE_SECS)
    if rows is None:
        raise SystemExit(forge_pr.UNREAD)
    merged = functools.cache(lambda: pr_review.holds_for(pr["number"], base))
    holds = lambda reviewed, head: pr_review.on_head(reviewed, head) or merged()(reviewed, head)
    found = facts(pr, notes, rows, holds, who, parent)
    return (*state_of(found), rows, found, holds)


def rows_mode(ask=False):
    """QUEUE_ROWS as off, shadow or on: the job's environment, else the forge's variable when `ask`, for a job
    that does not export it; any other word is said and read as off."""
    told = os.environ.get("QUEUE_ROWS", "")
    if not told and ask:
        answer = forge_pr.fgj_api("GET", f"repos/{forge_pr.repo()}/actions/variables/QUEUE_ROWS", timeout=FORGE_SECS)
        if isinstance(answer, dict) and "message" in answer:
            print(f"queue: the forge did not read QUEUE_ROWS ({answer['message']}), so the rows stay off")
        told = answer.get("data") or "" if isinstance(answer, dict) else ""
    if told not in ("", "off", "shadow", "on"):
        print(f"queue: QUEUE_ROWS reads {told!r}, none of off, shadow, on, so the rows stay off")
    return told if told in ("shadow", "on") else "off"


def parent_of(base, every):
    """Where a stacked PR's base branch stands, and the PR to re-point from: open; closed (its PR into main merged
    or closed, or the branch deleted); or branch, one no PR into main names, which only a person re-points."""
    repo = forge_pr.repo()
    if any(p["head"]["ref"] == base and (p["head"].get("repo") or {}).get("full_name") == repo for p in every):
        return "open", None
    path = urllib.parse.quote(base, safe="")
    found = forge_pr.fgj_api("GET", f"repos/{repo}/pulls/main/{path}", timeout=FORGE_SECS)
    if isinstance(found, dict) and found.get("number"):
        return ("open" if found.get("state") == "open" else "closed"), found
    if found is not None:
        raise SystemExit(f"the forge did not say which PR has the branch {base}: {found}")
    branch = forge_pr.fgj_api("GET", f"repos/{repo}/branches/{path}", timeout=FORGE_SECS)
    if branch is None:
        return "closed", {"head": {"ref": base, "repo": {"full_name": repo}}}
    if not isinstance(branch, dict) or branch.get("name") != base:
        raise SystemExit(f"the forge did not say whether the branch {base} exists: {branch}")
    return "branch", None


def marker(sha):
    return f"<!-- yi-unknown {sha} -->"


def owed(pr, state, notes, rows, holds, who):
    """The act the tick owes this PR now, or None: its state's act, refused for what the act itself refuses,
    so that shadow says only what `on` would do."""
    act = next((row[4] for row in STATES if row[0] == state), None)
    if act == "enqueue":
        # Invariant: the row enqueues PRs opened by the owner's login alone, a subset of what the Mac loop offers.
        mine = (pr.get("user") or {}).get("login") in MERGERS
        return act if mine and eligible_problem(pr, forge_pr.job_states(forge_pr.as_jobs(rows))) is None else None
    if act == "promote":
        return act if pr_review.promotes(pr_review.rounds_of(notes, who, pr["number"]), pr["head"]["sha"], holds) else None
    if act == "say":
        return None if any(marker(pr["head"]["sha"]) in (c.get("body") or "") for c in notes) else act
    return act


def do(pr, act, notes, who, found, parent):
    """Invariant: a label, title or base write carries no sha guard, so the act re-reads the head it judged and
    refuses a moved one; each act is a no-op once done, so a second tick repeats nothing."""
    number, judged = pr["number"], pr["head"]["sha"]
    fresh = forge_pr.pull(number)
    if fresh["head"]["sha"] != judged:
        print(f"queue: #{number} skipped {act}: its head moved from {judged[:10]} to {fresh['head']['sha'][:10]} after the judgement")
        return
    if act == "enqueue":
        if LABEL in {label["name"] for label in fresh.get("labels") or []}:
            print(f"queue: #{number} skipped enqueue: it already carries `{LABEL}`")
            return
        set_label(number, LABEL, True)
    elif act == "promote":
        pr_review.promote(forge_pr.repo(), fresh, pr_review.rounds_of(notes, who, number))
    elif act == "repoint":
        if fresh["base"]["ref"] == "main":
            return
        retarget(parent)
    else:
        shown = ", ".join(f"{k}={sorted(v) if isinstance(v, set) else v}" for k, v in found.items())
        comment(number, f"{marker(judged)}\nNo row of the queue's state table matches this pull request at {judged[:10]}, "
                        f"so no bot is due to act on it. What it read: {shown}.")
    print(f"queue: #{number} {act} at {judged[:10]}")


def tend(pr, state, since, rows, notes, holds, who, found, parent, mode):
    """Record the act this PR's state owes as its head's `yi/would`, and under `on` do it."""
    act = owed(pr, state, notes, rows, holds, who)
    when = datetime.datetime.fromtimestamp(since, datetime.timezone.utc).strftime("%m-%d %H:%M")
    # Invariant: no `yi/would` reads as nothing owed, so a head gets one only once an act was owed on it.
    if act or old_status(rows, WOULD):
        post(pr, rows, WOULD, f"would {act} ({state} since {when}Z)" if act else f"owes nothing since {when}Z ({state})")
    if act and mode == "on":
        do(pr, act, notes, who, found, parent)


def old_status(rows, context):
    return next((row.get("description") or "" for row in rows if row.get("context") == context), "")


def post(pr, rows, context, text):
    """Write `text` as the head's `context` status unless the last one said as much; True when it wrote."""
    if said(old_status(rows, context)) == said(text):
        return False
    answer = forge_pr.fgj_api("POST", f"repos/{forge_pr.repo()}/statuses/{pr['head']['sha']}",
                              {"state": "success", "context": context, "description": text}, timeout=FORGE_SECS)
    if not isinstance(answer, dict) or answer.get("context") != context:
        raise SystemExit(f"the forge refused the status: {answer}")
    return True


def report(every, held=None, mode="off"):
    """One `yi/state` status per open head under STATE_REPORT, and the rows' `yi/would` and acts under
    QUEUE_ROWS, each written only when what it says changed; `held` is the comments the tick read, by PR."""
    on = os.environ.get("STATE_REPORT") == "on"
    if not on and mode == "off":
        return
    now, wrote, who = time.time(), 0, pr_review.authors()
    for pr in every:
        try:
            notes = (held or {}).get(pr["number"])
            notes = pr_review.comments(forge_pr.repo(), pr["number"], FORGE_SECS) if notes is None else notes
            base = pr["base"]["ref"]
            parent = parent_of(base, every) if mode != "off" and base != "main" else (None, None)
            state, owner, since, clock, rows, found, holds = assess(pr, who, notes, parent[0])
            wrote += on and post(pr, rows, STATE, describe(state, owner, since, clock, now))
            if mode != "off":
                tend(pr, state, since, rows, notes, holds, who, found, parent[1], mode)
        # Invariant: one PR's forge error, a timeout included, never ends the tick or the report for the others.
        except (Exception, SystemExit) as err:
            print(f"queue: no state for #{pr['number']}: {err}"[:300])
    if on:
        print(f"queue: state written on {wrote} of {len(every)} open heads")


def cmd_parked(args):
    """Every open PR past its state's clock, oldest first, from the forge as it is now; writes nothing."""
    print(last_tick(time.time()))
    every, now, rows, who = listing(forge_pr.open_pulls(), "the open pull requests"), time.time(), [], pr_review.authors()
    for pr in every:
        try:
            state, owner, since, clock, *_ = assess(pr, who, pr_review.comments(forge_pr.repo(), pr["number"], FORGE_SECS))
        except (Exception, SystemExit) as err:
            print(f"#{pr['number']} unreadable: {err}"[:300])
            continue
        if (now - since) / 3600 > clock:
            rows.append((since, f"#{pr['number']} {(now - since) / 3600:.0f} h  {state}, waiting on {owner} (clock {clock:.3g} h)  {pr['title']}"))
    for _, row in sorted(rows):
        print(row)
    print(f"{len(rows)} of {len(every)} open pull requests past their clock")
    return 0


# Invariant: an autofix pass runs to its 90-minute timeout, so a run still going is alive that long; 45 minutes
# is three missed autofix crons, and the record's longest gap between passes that each ran was 92 minutes.
DARK_MINUTES, RUN_MINUTES, LOOKBACK_HOURS, PAGES = 45, 100, 6, 20


def job_records(watched, now, want):
    """`watched`'s job records, newest first, read page by page until `want(records)` holds, a record of any
    job is older than LOOKBACK_HOURS, or PAGES pages are read."""
    found, pages = [], []

    def transport(method, path, payload):
        answer = forge_pr.fgj_api(method, path, payload, timeout=FORGE_SECS)
        rows = answer.get("workflow_runs") if isinstance(answer, dict) else None
        if rows is None:
            raise SystemExit(f"the forge did not list the job records: {answer}")
        pages.append(path)
        found.extend(r for r in rows if r.get("workflow_id") == watched)
        old = any(now - pr_review.when(r["created_at"]) > LOOKBACK_HOURS * 3600 for r in rows)
        return [] if want(found) or old or len(pages) >= PAGES else rows

    list(forge_tracking.paged(transport, f"repos/{forge_pr.repo()}/actions/tasks?x=1"))
    return sorted(found, key=lambda r: r["created_at"], reverse=True)


def finished(records):
    return [r for r in records if r["status"] in ("success", "failure")]


def dark(records, now):
    """Why a scheduled job is dark at `now`, from its records newest first; None while it runs."""
    done = finished(records)
    if len(done) >= 3 and all(r["status"] == "failure" for r in done[:3]):
        return "its last three runs failed"
    if not records:
        return f"it has no run in the last {LOOKBACK_HOURS} h"
    newest = records[0]
    limit = RUN_MINUTES if newest["status"] in ("running", "waiting") else DARK_MINUTES
    return f"it has not run since {newest['created_at']}" if (now - pr_review.when(newest["created_at"])) / 60 > limit else None


def heartbeat(watched, mode=None):
    """Keep one issue open while the other scheduled job is dark and close it once that job runs green: the queue
    tick watches the fixer's pass and the pass the tick, so a dead job is reported by the live one."""
    import pr_autofix

    try:
        if (rows_mode(ask=True) if mode is None else mode) == "off":
            return
        now, repo, title = time.time(), forge_pr.repo(), f"The {watched} job is dark"
        records = job_records(watched, now, lambda found: len(finished(found)) >= 3)
        why, have = dark(records, now), pr_autofix.red_issue(repo, None, title)
        if have is False:
            raise SystemExit("the forge did not list the open issues")
        body = f"The scheduled `{watched}` job is dark: {why}. Its newest records:\n\n" + "".join(
            f"- {r['created_at']} {r['status']}\n" for r in records[:5]) if why else ""
        if why and not have:
            ids = label_ids()
            stones = api("GET", "milestones?state=open&limit=50") or []
            stone = next((m["id"] for m in stones if isinstance(m, dict) and m.get("title") == "Forge plumbing"), None)
            made = api("POST", "issues", {"title": title, "body": body, "milestone": stone,
                                          "labels": [ids[n] for n in ("size:S", "area:forge-ci") if n in ids]})
            print(f"heartbeat: {watched} is dark ({why}); opening its issue answered {(made or {}).get('number') or made}")
        elif why and why not in (have.get("body") or ""):
            api("PATCH", f"issues/{have['number']}", {"body": body})
        elif have and not why and finished(records)[:1] and finished(records)[0]["status"] == "success":
            api("PATCH", f"issues/{have['number']}", {"state": "closed"})
            print(f"heartbeat: {watched} ran green again; #{have['number']} closed")
    except (Exception, SystemExit) as err:
        print(f"heartbeat: {watched} unread, so nothing is said of it: {err}"[:300])


def last_tick(now):
    """The first line of `just pr parked`: how long ago the queue's tick last ran to success."""
    try:
        ok = [r for r in job_records("queue.yml", now, lambda found: any(r["status"] == "success" for r in found)) if r["status"] == "success"]
    except SystemExit as err:
        return f"last successful queue tick: unread ({err})"
    if not ok:
        return f"last successful queue tick: none in the job records of the last {LOOKBACK_HOURS} h"
    return f"last successful queue tick: {(now - pr_review.when(ok[0]['created_at'])) / 60:.0f} min ago, at {ok[0]['created_at']}"


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


def enqueue_time(events, rowed=lambda: False):
    """When a merger last added the label; None when nobody allowed did, or it has since come off.
    The forge lets any writer add one, and the timeline's body is "1" for an add. The bot's own add counts
    only while `rowed()`, which asks whether the enqueue row would add it now."""
    at = None
    for event in events:
        if event.get("type") == "label" and (event.get("label") or {}).get("name") == LABEL:
            login = (event.get("user") or {}).get("login")
            allowed = login in MERGERS or (login == pr_review.BOT and event.get("body") == "1" and rowed())
            at = event.get("created_at") if event.get("body") == "1" and allowed else None
    return at


def rowed(number):
    """Invariant: a bot-added label is the owner's consent only through the enqueue row, so it counts while
    QUEUE_ROWS is on and the row, judging the PR as if unlabelled, would add it at this moment."""
    if rows_mode(ask=True) != "on":
        return False
    pr = forge_pr.pull(number)
    bare = {**pr, "labels": [label for label in pr.get("labels") or [] if label["name"] != LABEL]}
    who, notes = pr_review.authors(), pr_review.comments(forge_pr.repo(), number, FORGE_SECS)
    state, _, _, _, rows, _, holds = assess(bare, who, notes)
    return owed(bare, state, notes, rows, holds, who) == "enqueue"


def enqueued_at(number):
    transport = lambda method, url, _: listing(forge_pr.fgj_api(method, url) or [], f"#{number}'s timeline")
    events = forge_tracking.paged(transport, f"repos/{forge_pr.repo()}/issues/{number}/timeline?x=1")
    return enqueue_time(events, functools.cache(lambda: rowed(number)))


def queued_pulls(every=None):
    every = listing(forge_pr.open_pulls(), "the open pull requests") if every is None else every
    return [pr for pr in every if LABEL in {label["name"] for label in pr.get("labels") or []}]


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


def dequeue_reason(pr, notes):
    if "autofix:failed" in {label["name"] for label in pr.get("labels") or []}:
        return "the fixer gave up on it (`autofix:failed`)"
    rounds = pr_review.rounds_of(notes, pr_review.authors(), pr["number"])
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
    every = listing(forge_pr.open_pulls(), "the open pull requests")
    pulls, notes = queued_pulls(every), {}
    for pr in list(pulls):
        if "autofix:failed" not in {label["name"] for label in pr.get("labels") or []}:
            notes[pr["number"]] = pr_review.comments(forge_pr.repo(), pr["number"])
        if reason := dequeue_reason(pr, notes.get(pr["number"], [])):
            set_label(pr["number"], LABEL, False)
            comment(pr["number"], f"Left the queue: {reason}. Add the `{LABEL}` label again once that is answered.")
            pulls.remove(pr)
    obs = {"candidate": None, "in_main": False, "base": None, "main": git("rev-parse", "origin/main").stdout.strip(),
           "stale_member": None, "lanes": {}, "age_minutes": 0, "retries": 0, "members": [], "queued": [], "pulls": pulls, "open": every, "notes": notes}
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


def retarget(pr):
    """Point the open PRs based on a landed PR's branch at main and merge main into them, which reruns their checks.
    Incident: thirteen PRs stacked on each other's branches were refused as not based on main, for good."""
    head = pr.get("head") or {}
    branch = head.get("ref")
    # Invariant: only a branch of this repository is a base here; a fork's head can carry the same name.
    if not branch or branch == "main" or (head.get("repo") or {}).get("full_name") != forge_pr.repo():
        return
    for child in listing(forge_pr.open_pulls(), "the open pull requests"):
        number = child["number"]
        if child["base"]["ref"] != branch:
            continue
        moved = api("PATCH", f"pulls/{number}", {"base": "main"}) or {}
        if (moved.get("base") or {}).get("ref") != "main":
            comment(number, f"The queue landed `{branch}` but the forge refused to move this pull request's base to main ({moved.get('message')}). Change the base by hand.")
            continue
        asked = argparse.Namespace(number=str(number))
        try:
            # Incident: a rerun of #2237's old head stayed red on an advisory main had patched; only main merged in clears it.
            # Invariant: the forge answers an update of a current branch with success and no new head, so nothing reran.
            # Invariant: the bot's token never writes to a fork's branch, so a fork's child is re-pointed and rerun only.
            ours = (child["head"].get("repo") or {}).get("full_name") == forge_pr.repo()
            stale = not ours or forge_pr.cmd_update(asked) or forge_pr.pull(number)["head"]["sha"] == child["head"]["sha"]
            failed = stale and forge_pr.cmd_rerun(asked)
        except SystemExit as err:
            failed = err
        if failed:
            comment(number, f"The queue landed `{branch}` and moved this pull request's base to main, but main was not merged in and its checks did not rerun. Run `just pr update {number}`.")


def finish(obs, recorded=False):
    """Idempotent, so a tick killed after the push does this through the `landed` rule instead."""
    for number, _ in obs["members"]:
        pr = forge_pr.pull(number)
        retarget(pr)
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
    report(obs["open"], obs["notes"], rows_mode())
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
                                 queued_pulls=lambda every=None: [pulls[number] for number in sorted(queued)], enqueued_at=lambda number: number,
                                 dequeue_reason=lambda pr, notes: None, eligible_problem=lambda pr, jobs: None,
                                 record_candidate=lambda candidate: (candidate, True), use_driver=lambda cwd: None,
                                 comment=lambda number, text: comments.append((number, text)), set_label=label, delete_ref=remove,
                                 statuses_url=lambda sha, lane: ""), \
                mock.patch.multiple(forge_pr, pull=lambda number: pulls[number], is_behind=lambda pr: False, repo=lambda: "r",
                                    head_jobs=jobs, unreviewed=lambda number, pr: [], open_pulls=lambda: []), \
                mock.patch.object(pr_review, "comments", lambda repo, number: []):
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

        # Incident: the race hung on a pre-push hook whose own push could fail unfelt, so the check read a moved main
        # only when that push happened to win; the recorder now moves main itself, every run.
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
        script.write_text(RECORDER)
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
                                    record_main=lambda: real_record_main(root), set_label=mock.DEFAULT, delete_ref=mock.DEFAULT, retarget=mock.DEFAULT):
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
           "members": list(heads.items()), "stale_member": None, "open": [], "notes": {}}
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
        answers = lambda method, path, payload=None: locked if "/comments" in path else [queued] if "page=1" in path else []
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
        # A landed branch's children move to main and take main in; a PR based elsewhere is not touched.
        bases = ((8, "b7"), (9, "main"), (10, "b9"))
        # A refused move is said on the child; an update refused (1) or with nothing to merge (2) falls back to a rerun.
        on_main = {"base": {"ref": "main"}}
        for owner, answer, update, rerun, want, kid in (("o/r", on_main, 0, 0, (["8"], ["8"], [], 0), "o/r"), ("o/r", {"message": "409"}, 0, 0, (["8"], [], [], 1), "o/r"),
                                                   ("o/r", on_main, 1, 0, (["8"], ["8"], ["8"], 0), "o/r"), ("o/r", on_main, 1, 1, (["8"], ["8"], ["8"], 1), "o/r"), ("o/r", on_main, 2, 0, (["8"], ["8"], ["8"], 0), "o/r"),
                                                   ("o/r", on_main, 0, 0, (["8"], [], ["8"], 0), "fork/r"),
                                                   ("fork/r", on_main, 0, 0, ([], [], [], 0), "o/r")):
            moved, updated, reran = [], [], []
            pulls = [{"number": n, "base": {"ref": ref}, "head": {"sha": "old", "repo": {"full_name": kid}}} for n, ref in bases]
            with mock.patch.object(forge_pr, "fgj_api", lambda method, path, payload=None: (moved.append((path.rsplit("/", 1)[1], payload)), answer)[1] if method == "PATCH" and "/pulls/" in path else {"body": ""}), \
                    mock.patch.object(forge_pr, "pull", lambda n: {"body": "", "head": {"ref": "b7", "sha": "old" if update else "new", "repo": {"full_name": owner}}}), mock.patch.object(forge_pr, "open_pulls", lambda: pulls), \
                    mock.patch.object(forge_pr, "repo", lambda: "o/r"), mock.patch.object(forge_pr, "cmd_rerun", lambda ns: reran.append(ns.number) or rerun), \
                    mock.patch.object(forge_pr, "cmd_update", lambda ns: updated.append(ns.number) or update == 1), \
                    mock.patch.multiple(sys.modules[__name__], delete_ref=mock.DEFAULT, record_main=mock.DEFAULT, set_label=mock.DEFAULT, comment=mock.DEFAULT) as stubs:
                with contextlib.redirect_stdout(io.StringIO()):
                    finish({"members": [(7, "a" * 40)], "age_minutes": 1})
            got = ([n for n, payload in moved if payload == {"base": "main"}], updated, reran, stubs["comment"].call_count)
            if got != want or len(moved) != len(want[0]):
                bad.append(f"landing b7 of {owner} with a child of {kid}, the move answered {answer}, the update {update} and the rerun {rerun}: moved {moved}, updated {updated}, reran {reran}, {got[3]} comment(s), not {want}")
            said = str(stubs["comment"].call_args)
            if update and rerun and ("just pr update 8" not in said or "just pr rerun" in said):
                bad.append(f"a child neither updated nor rerun was not told to run `just pr update`: {said}")
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


RECORDED = ROOT / "scripts/fixtures/pr_state/recorded.json"
FACTS = {"labels": set(), "draft": False, "base": "main", "gate": "green", "mergeable": True, "since": 0.0,
         "fix": None, "lost": False, "retries_spent": False, "verdict": "clean"}
# The states the recording holds no gap of: no gap over a day sat in them, or the recording cannot hold the fact.
STATE_CASES = (
    ("fixer working", dict(labels={"autofix:working"}, verdict="blocked")),
    ("lost answers, retries spent", dict(lost=True, retries_spent=True, verdict=None)),
    ("blocked round, queued", dict(labels={"queue"}, verdict="blocked")),
    ("conflicted", dict(mergeable=False)),
    ("gate pending", dict(gate="pending")),
    ("queued", dict(labels={"queue"})),
    ("unknown", dict(verdict="none such")),
)


def state_cases(off=""):
    """Each recorded parked moment reads the state it was recorded in, and each other state its hand case."""
    bad = []
    for case in json.loads(RECORDED.read_text()):
        got = state_of(facts(case["pr"], case["comments"], case["statuses"], pr_review.on_head, {"yi-bot", "jack"}), off)[0]
        if got != case["state"]:
            bad.append(f"#{case['pr']['number']} at {case['at']}, {case['b_state']} for {case['gap_hours']} h, read {got}, not {case['state']}")
    for case in json.loads(ROWS_RECORDED.read_text()):
        parent = "closed" if case.get("parent") else None
        found = facts(case["pr"], case["comments"], case["statuses"], pr_review.on_head, {"yi-bot", "jack"}, parent)
        if case["state"] != "unknown" and (got := state_of(found, off)[0]) != case["state"]:
            bad.append(f"{case['case']} read {got}, not {case['state']}")
    for want, change in STATE_CASES:
        if (got := state_of({**FACTS, **change}, off)[0]) != want:
            bad.append(f"{change} read {got}, not {want}")
    return bad


def forge_double(statuses, refuse=(), pulls=(), hang=(), tasks=()):
    """A forge for the tick and the report: the open listing, comment and timeline pages, combined statuses by
    head, status posts kept, job records; a path holding a `refuse` word answers an error, a `hang` word times out."""
    calls = []

    def answer(method, path, payload=None, timeout=None):
        calls.append((method, path, payload, timeout))
        if any(word in path for word in hang):
            raise subprocess.TimeoutExpired("fgj", timeout)
        if any(word in path for word in refuse):
            return {"message": "500 database is locked"}
        sha = path.split("/")[-2] if path.endswith("/status") else path.rsplit("/", 1)[-1]
        if method == "GET" and "/pulls?state=open" in path:
            return list(pulls)
        if method == "GET" and ("/comments?" in path or "/timeline?" in path):
            return []
        if method == "GET" and path.endswith("/status"):
            return {"statuses": statuses.get(sha, [])}
        if method == "GET" and "/actions/tasks?" in path:
            return {"workflow_runs": list(tasks) if path.endswith("page=1") else []}
        if method == "POST" and "/statuses/" in path:
            statuses.setdefault(sha, [])[:] = [s for s in statuses.get(sha, []) if s["context"] != payload["context"]] + [
                {"context": payload["context"], "status": payload["state"], "description": payload["description"]}]
            return payload
        raise AssertionError(f"the report made a call it may not: {method} {path}")
    return answer, calls


def report_cases():
    """The report writes one success status per head, again only on a change, past one PR's error, and only when on."""
    bad, now = [], time.time()
    pr = lambda n, sha: {"number": n, "title": "t", "labels": [], "base": {"ref": "main"}, "head": {"sha": sha},
                         "mergeable": True, "updated_at": datetime.datetime.fromtimestamp(now - 3600, datetime.timezone.utc).isoformat()}
    every = [pr(1, "a" * 40), pr(2, "b" * 40), pr(3, "c" * 40), pr(4, "d" * 40), pr(5, "e" * 40)]
    statuses = {}
    answer, calls = forge_double(statuses, refuse={"b" * 40, "issues/4/"}, hang={"e" * 40})
    with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "o/r"), \
            mock.patch.dict(os.environ, {"YI_ROUND_AUTHORS": "yi-bot"}), contextlib.redirect_stdout(io.StringIO()) as said_out:
        with mock.patch.dict(os.environ, {"STATE_REPORT": "off"}):
            report(every)
        if calls:
            bad.append(f"a report switched off made {len(calls)} forge calls")
        with mock.patch.dict(os.environ, {"STATE_REPORT": "on"}):
            report(every)
            posts = [c for c in calls if c[0] == "POST"]
            if sorted(c[1].rsplit("/", 1)[1][0] for c in posts) != ["a", "c"] or any(c[2]["state"] != "success" for c in posts):
                bad.append(f"one head's refused read did not leave the others written, each success: {posts}")
            if not all(f"no state for #{n}" in said_out.getvalue() for n in (2, 4, 5)):
                bad.append(f"a refused read went unsaid: {said_out.getvalue()!r}")
            if any(c[3] is None for c in calls):
                bad.append(f"a report call had no timeout: {[c for c in calls if c[3] is None]}")
            if "awaiting review, waiting on review" not in str(statuses.get("a" * 40)):
                bad.append(f"a head with no lane run read {statuses.get('a' * 40)}")
            before = len(calls)
            report(every)
            if [c for c in calls[before:] if c[0] == "POST"]:
                bad.append("an unchanged state was written again, the 272-comment loop as statuses")
            with mock.patch.object(time, "time", lambda: now + 3600 * 20):
                before = len(calls)
                report(every)
                crossed = [c for c in calls[before:] if c[0] == "POST"]
            if len(crossed) != 2 or not all(c[2]["description"].endswith(" parked") for c in crossed):
                bad.append(f"crossing the clock did not write each head parked once: {crossed}")
    return bad


def aged(n, sha, hours, now, labels=()):
    return {"number": n, "title": f"PR {n}", "labels": [{"name": name} for name in labels], "base": {"ref": "main"},
            "head": {"sha": sha}, "mergeable": True,
            "updated_at": datetime.datetime.fromtimestamp(now - hours * 3600, datetime.timezone.utc).isoformat()}


def tick_call_cases():
    """A tick reads a queued PR's comments once with the report on or off, and off it reads nothing else of an
    unqueued one; under MERGE_QUEUE off the report alone runs, and with both off nothing is asked."""
    bad, now = [], time.time()
    pulls = [aged(1, "a" * 40, 1, now, ["queue"]), aged(2, "b" * 40, 1, now)]
    seen = {}
    for switch in ("off", "on"):
        answer, calls = forge_double({}, pulls=pulls)
        with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "o/r"), \
                mock.patch.object(sys.modules[__name__], "git", mock.Mock(return_value=mock.Mock(stdout="", returncode=0))), \
                mock.patch.dict(os.environ, {"STATE_REPORT": switch, "YI_ROUND_AUTHORS": "yi-bot"}), contextlib.redirect_stdout(io.StringIO()):
            tick("on")
        seen[switch] = calls
        if switch == "off" and [c for c in calls if "/issues/2/" in c[1] or "b" * 40 in c[1]]:
            bad.append(f"a tick with the report off read the unqueued PR: {calls}")
    # Invariant: QUEUE_ROWS off, set or unset, leaves the tick's calls those of the tick before the rows existed.
    answer, calls = forge_double({}, pulls=pulls)
    with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "o/r"), \
            mock.patch.object(sys.modules[__name__], "git", mock.Mock(return_value=mock.Mock(stdout="", returncode=0))), \
            mock.patch.dict(os.environ, {"MERGE_QUEUE": "off", "STATE_REPORT": "off", "QUEUE_ROWS": "off", "YI_ROUND_AUTHORS": "yi-bot"}), \
            contextlib.redirect_stdout(io.StringIO()):
        main([])
        tick("on")
    if calls != seen["off"]:
        bad.append(f"QUEUE_ROWS off changed the tick's forge calls: {calls} against {seen['off']}")
    comments = {switch: [c[1] for c in calls if "/issues/1/comments" in c[1]] for switch, calls in seen.items()}
    if comments["on"] != comments["off"]:
        bad.append(f"the report read the queued PR's comments again: {comments}")
    if len([c for c in seen["off"] if c[0] != "GET"]) or len(seen["on"]) <= len(seen["off"]):
        bad.append(f"the report switch did not decide the extra calls: off {len(seen['off'])}, on {len(seen['on'])}")
    for queue, switch, want in (("off", "on", 2), ("off", "off", 0)):
        answer, calls = forge_double({}, pulls=pulls)
        with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "o/r"), \
                mock.patch.object(sys.modules[__name__], "tick", mock.Mock(return_value=0)) as ticked, \
                mock.patch.dict(os.environ, {"MERGE_QUEUE": queue, "STATE_REPORT": switch, "YI_ROUND_AUTHORS": "yi-bot"}), \
                contextlib.redirect_stdout(io.StringIO()):
            main([])
        posts = [c for c in calls if c[0] == "POST"]
        if ticked.called or len(posts) != want or (not want and calls):
            bad.append(f"MERGE_QUEUE {queue} with STATE_REPORT {switch} ticked={ticked.called}, posted {len(posts)} of {want}, calls {len(calls)}")
    return bad


def parked_cases():
    """`just pr parked` prints the PRs past their clock, oldest first, and never one within it; it writes nothing."""
    now = time.time()
    pulls = [aged(1, "a" * 40, 30, now), aged(2, "b" * 40, 1, now), aged(3, "c" * 40, 40, now), aged(4, "d" * 40, 50, now)]
    stamp = lambda minutes: datetime.datetime.fromtimestamp(now - minutes * 60, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    tasks = [{"workflow_id": "queue.yml", "status": "failure", "created_at": stamp(2)},
             {"workflow_id": "autofix.yml", "status": "success", "created_at": stamp(4)},
             {"workflow_id": "queue.yml", "status": "success", "created_at": stamp(7)}]
    answer, calls = forge_double({}, pulls=pulls, hang={"issues/4/"}, tasks=tasks)
    with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "o/r"), \
            mock.patch.dict(os.environ, {"YI_ROUND_AUTHORS": "yi-bot"}), contextlib.redirect_stdout(io.StringIO()) as out:
        cmd_parked(argparse.Namespace())
    printed = [line.split()[0] for line in out.getvalue().splitlines() if " h  " in line]
    bad = [] if printed == ["#3", "#1"] else [f"parked printed {printed}, not ['#3', '#1']: {out.getvalue()!r}"]
    if not out.getvalue().startswith(f"last successful queue tick: 7 min ago, at {stamp(7)}\n"):
        bad.append(f"parked did not open on the last successful tick's age: {out.getvalue()[:120]!r}")
    if "#4 unreadable" not in out.getvalue():
        bad.append(f"a PR whose comments timed out was not named unreadable: {out.getvalue()!r}")
    return bad + [f"parked wrote {c}" for c in calls if c[0] != "GET"]


def neutral_cases():
    """A `yi/state` success on a head moves no decision that reads the head's statuses."""
    import pr_autofix
    bad = []
    lanes = lambda *states: [{"context": f"pr / {job} (pull_request)", "status": state} for job, state in zip((*LANES, "title"), states)]
    mine = {"context": STATE, "status": "success", "description": "unknown, waiting on person since 11-22 10:04Z (26 h, clock 0 h) parked"}
    pr = {"number": 7, "title": "t", "state": "open", "head": {"sha": "a" * 40}, "base": {"ref": "main"}, "labels": []}
    for rows in ([], lanes("success", "success", "success", "success"), lanes("success", "failure", "pending"), lanes("pending")):
        seen = []
        would = {"context": WOULD, "status": "success", "description": "would enqueue (clean, not queued since 11-22 10:04Z)"}
        for extra in ([], [mine], [mine, would]):
            with mock.patch.object(forge_pr, "fgj_api", lambda *a, **k: {"statuses": rows + extra}), \
                    mock.patch.object(forge_pr, "repo", lambda: "o/r"), mock.patch.object(forge_pr, "required", lambda: [*LANES, "title"]):
                jobs = forge_pr.jobs_of(pr)
                seen.append((forge_pr.decide(pr, jobs, [*LANES, "title"], False), lanes_state({"lanes": jobs}),
                             pr_autofix.red_lanes("o/r", pr["head"]["sha"]), forge_pr.rerun_waits(pr),
                             facts(pr | {"updated_at": "2026-11-22T10:04:00Z"}, [], rows + extra, pr_review.on_head, {"yi-bot"})))
        if any(found != seen[0] for found in seen):
            bad.append(f"a yi/state or yi/would status moved a decision on {rows}: {seen}")
    return bad


ROWS_RECORDED = ROOT / "scripts/fixtures/queue_rows/recorded.json"
JOBS_RECORDED = ROOT / "scripts/fixtures/queue_rows/tasks.json"


def rows_forge(cases, moved=(), refuse=(), rebased=()):
    """A forge holding recorded PRs as the rows read and write them: each write kept in `writes` and applied, a PR
    in `moved` re-read with a new head and one in `rebased` on main, a path holding a `refuse` word answering an error."""
    world = json.loads(json.dumps({str(c["pr"]["number"]): c for c in cases}))
    writes, ids = [], {"queue": 1, "autofix": 2}
    case = lambda n: world[str(n)]

    def answer(method, path, payload=None, timeout=None):
        rest = path.split("/", 3)[3]
        if any(word in path for word in refuse):
            return {"message": "500 database is locked"}
        if method != "GET":
            writes.append((method, rest, payload))
        if rest.startswith("pulls?state=open"):
            return [c["pr"] for c in world.values()] if rest.endswith("page=1") else []
        if rest.startswith("pulls/main/"):
            return next((c["parent"] for c in world.values() if c.get("parent", {}).get("head", {}).get("ref") == urllib.parse.unquote(rest[11:])), None)
        if rest.startswith("branches/"):
            return {"name": urllib.parse.unquote(rest[9:])}
        if rest.startswith("labels?"):
            return [{"name": name, "id": i} for name, i in ids.items()]
        if rest.startswith("commits/"):
            return {"statuses": next((c["statuses"] for c in world.values() if c["pr"]["head"]["sha"] == rest.split("/")[1]), [])}
        if rest.startswith("statuses/"):
            for c in world.values():
                if c["pr"]["head"]["sha"] == rest.split("/")[1]:
                    c["statuses"] = [s for s in c["statuses"] if s["context"] != payload["context"]] + [
                        {"context": payload["context"], "status": payload["state"], "description": payload["description"]}]
            return payload
        number = int(rest.split("/")[1].split("?")[0])
        if rest.startswith("pulls/") and method == "GET":
            pr = json.loads(json.dumps(case(number)["pr"]))
            pr = {**pr, "base": {"ref": "main"}} if number in rebased else pr
            return {**pr, "head": {**pr["head"], "sha": "f" * 40}} if number in moved else pr
        if rest.startswith("pulls/"):
            pr = case(number)["pr"]
            pr.update({k: v for k, v in payload.items() if k == "title"})
            pr["base"] = {"ref": payload["base"]} if "base" in payload else pr["base"]
            return pr
        if rest.endswith("/labels"):
            case(number)["pr"]["labels"].append({"name": next(n for n, i in ids.items() if i in payload["labels"])})
            return []
        if "/comments" in rest and method == "POST":
            case(number)["comments"].append({"id": 10**9 + len(writes), "user": {"login": pr_review.BOT}, "body": payload["body"]})
            return {"id": 1}
        if "/comments" in rest:
            return case(number)["comments"] if rest.endswith("page=1") else []
        raise AssertionError(f"the rows made a call they may not: {method} {path}")
    return answer, writes, world


@contextlib.contextmanager
def rows_world(cases, mode, moved=(), refuse=(), report_on="off", rebased=()):
    answer, writes, world = rows_forge(cases, moved, refuse, rebased)
    on_head = lambda number, base="main": pr_review.on_head
    # The recording holds no PR that matches no row, so the stand-in's own label is put first for the last row's act.
    odd = ("unknown", "person", 0, lambda f: "stand-in:unknown" in f["labels"], STATES[-1][4])
    with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "apex/yi"), \
            mock.patch.object(sys.modules[__name__], "STATES", (odd, *STATES)), mock.patch.object(pr_review, "holds_for", on_head), mock.patch.object(forge_pr, "cmd_update", lambda ns: 0), \
            mock.patch.object(forge_pr, "cmd_rerun", lambda ns: 0), mock.patch.object(forge_pr, "required", lambda: [*LANES, "title"]), \
            mock.patch.dict(os.environ, {"YI_ROUND_AUTHORS": "yi-bot,jack", "STATE_REPORT": report_on, "QUEUE_ROWS": mode}), \
            contextlib.redirect_stdout(io.StringIO()) as said_out:
        yield (lambda: report([c["pr"] for c in world.values()], mode=mode)), writes, said_out


def rows_cases():
    """Recorded moments through the rows: shadow writes `yi/would` alone and once, `on` does each owed act once
    and only that, a moved head or a label already there is left alone, and one PR's error spares the rest."""
    cases, bad = json.loads(ROWS_RECORDED.read_text()), []
    owing = {c["pr"]["number"]: c["act"] for c in cases if c.get("act")}
    kinds = {"enqueue": ("POST", "/labels"), "promote": ("PATCH", "pulls/"), "repoint": ("PATCH", "pulls/"), "say": ("POST", "/comments")}
    acted = lambda writes: {int(w[1].split("/")[1]) for w in writes if not w[1].startswith("statuses/")}
    for mode in ("shadow", "on"):
        with rows_world(cases, mode) as (tick_rows, writes, said_out):
            tick_rows()
            first = list(writes)
            tick_rows()
        would = {c["pr"]["number"]: s["description"] for c in cases for s in c["statuses"] if s["context"] == WOULD}
        if mode == "shadow":
            if [w for w in first if w[0] != "POST" or not w[1].startswith("statuses/") or w[2]["context"] != WOULD]:
                bad.append(f"shadow wrote more than yi/would: {[w for w in first if not w[1].startswith('statuses/')]}")
            got = {n: d.split(" (")[0].removeprefix("would ") for n, d in would_of(first).items()}
            if got != owing:
                bad.append(f"shadow recorded {got}, not the owed {owing}")
        else:
            for n, act in owing.items():
                method, part = kinds[act]
                if not [w for w in first if w[0] == method and part in w[1] and f"/{n}" in w[1]]:
                    bad.append(f"on did not {act} #{n}: {[w for w in first if not w[1].startswith('statuses/')]}")
            if acted(first) - set(owing) - {c["pr"]["number"] for c in cases if c.get("act") == "repoint"}:
                bad.append(f"on wrote to a PR that owed nothing: {acted(first) - set(owing)}")
        done = {(w[0], w[1]) for w in first if not w[1].startswith("statuses/")}
        again = [w for w in writes[len(first):] if (w[0], w[1]) in done]
        if again or (mode == "shadow" and writes[len(first):]):
            bad.append(f"a second {mode} tick wrote again: {writes[len(first):]}")
    enqueue = next(n for n, act in owing.items() if act == "enqueue")
    with rows_world(cases, "on", moved={enqueue}) as (tick_rows, writes, said_out):
        tick_rows()
    if [w for w in writes if w[1].endswith("/labels")] or "head moved" not in said_out.getvalue():
        bad.append(f"a head that moved after the judgement was enqueued anyway: {writes}, {said_out.getvalue()!r}")
    raced = [c if c["pr"]["number"] != enqueue else {**c, "pr": {**c["pr"], "labels": c["pr"]["labels"] + [{"name": LABEL}]}} for c in cases]
    with rows_world(raced, "on") as (tick_rows, writes, said_out), mock.patch.object(sys.modules[__name__], "owed", lambda *a: "enqueue"):
        tick_rows()
    if [w for w in writes if w[1].endswith(f"{enqueue}/labels")]:
        bad.append(f"a PR already carrying `{LABEL}` from the Mac loop was labelled again: {writes}")
    with rows_world(cases, "on", refuse={f"issues/{enqueue}/comments"}) as (tick_rows, writes, said_out):
        tick_rows()
    if acted(writes) != set(owing) - {enqueue} or f"no state for #{enqueue}" not in said_out.getvalue():
        bad.append(f"one PR's refused read stopped the others: acted on {acted(writes)}, {said_out.getvalue()!r}")
    child = next(c for c in cases if c.get("act") == "repoint")
    with rows_world(cases, "on", rebased={child["pr"]["number"]}) as (tick_rows, writes, said_out):
        tick_rows()
    if [w for w in writes if w[0] == "PATCH" and "base" in (w[2] or {})]:
        bad.append(f"a child the queue's own landing re-pointed in the meantime was re-pointed again: {writes}")
    parent = child["parent"]
    open_parent = {"case": "the parent still open", "pr": {**parent, "state": "open", "title": "t", "labels": [], "base": {"ref": "main"},
                   "head": {**parent["head"], "sha": "3" * 40}, "user": {"login": "jack"}, "updated_at": child["pr"]["updated_at"]},
                   "comments": [], "statuses": []}
    with rows_world([child, open_parent], "on") as (tick_rows, writes, said_out):
        tick_rows()
    if [w for w in writes if not w[1].startswith("statuses/")]:
        bad.append(f"a child whose parent is still open was acted on: {writes}")
    stranger = [{**c, "pr": {**c["pr"], "user": {"login": "someone"}}} for c in cases if c.get("act") == "enqueue"]
    with rows_world(stranger, "on") as (tick_rows, writes, said_out):
        tick_rows()
    if writes:
        bad.append(f"a clean PR from a login outside MERGERS was acted on: {writes}")
    return bad


def would_of(writes):
    return {int(next(c for c in json.loads(ROWS_RECORDED.read_text()) if c["pr"]["head"]["sha"] == w[1].split("/")[1])["pr"]["number"]): w[2]["description"]
            for w in writes if w[1].startswith("statuses/") and w[2]["context"] == WOULD}


def consent_cases():
    """A bot-added `queue` label is consent only while QUEUE_ROWS is on and the row would add it now."""
    cases, bad = json.loads(ROWS_RECORDED.read_text()), []
    enqueue = next(c["pr"]["number"] for c in cases if c.get("act") == "enqueue")
    held = next(c["pr"]["number"] for c in cases if c["state"] == "held")
    added = lambda who: [{"type": "label", "label": {"name": LABEL}, "user": {"login": who}, "body": "1", "created_at": "t"}]
    for mode, number, want in (("on", enqueue, "t"), ("shadow", enqueue, None), ("off", enqueue, None), ("on", held, None)):
        with rows_world(cases, mode) as _:
            got = enqueue_time(added(pr_review.BOT), lambda: rowed(number))
        if got != want:
            bad.append(f"a bot-added label on #{number} under QUEUE_ROWS {mode} read {got}, not {want}")
    if enqueue_time(added("someone"), lambda: True) is not None:
        bad.append("a label from a login neither in MERGERS nor the bot's was honoured")
    return bad


def heartbeat_cases():
    """The recorded dark windows: the queue's tick dark by 11:18Z on 11-04 and by 07:15Z on 11-18 as the fixer's
    pass would read it, alive again once it ran green; the issue opens once, is not rewritten, and closes."""
    recs, bad, when = json.loads(JOBS_RECORDED.read_text()), [], pr_review.when
    seen = lambda job, t: [dict(r, status=r["status"] if when(r["updated_at"]) <= t else "running") for r in recs
                           if r["workflow_id"] == job and when(r["created_at"]) <= t]
    for at, job, want in (("2026-11-04T11:00:37Z", "queue.yml", None), ("2026-11-04T11:18:27Z", "queue.yml", "its last three runs failed"),
                          ("2026-11-04T17:00:36Z", "queue.yml", None), ("2026-11-18T07:15:37Z", "queue.yml", "its last three runs failed"),
                          ("2026-11-18T12:00:37Z", "queue.yml", None), ("2026-11-04T11:35:39Z", "autofix.yml", "its last three runs failed"),
                          ("2026-11-04T10:30:00Z", "autofix.yml", None)):
        if (got := dark(seen(job, when(at)), when(at))) != want:
            bad.append(f"{job} at {at} read dark={got!r}, not {want!r}")
    running = [{"created_at": "2026-11-04T09:00:00Z", "status": "running"}]
    if dark(running, when("2026-11-04T10:30:00Z")) or not dark(running, when("2026-11-04T11:00:00Z")):
        bad.append("a pass still running inside its 90-minute timeout read dark, or one past it read alive")
    issues, calls = [], []
    def answer(method, path, payload=None, timeout=None):
        calls.append((method, path))
        rest = path.split("/", 3)[3]
        if rest.startswith("actions/tasks"):
            return {"workflow_runs": state["records"] if rest.endswith("page=1") else []}
        if rest.startswith("issues?state=open"):
            return [i for i in issues if i["state"] == "open"]
        if rest.startswith(("labels?", "milestones?")):
            return [{"name": "area:forge-ci", "id": 5}, {"name": "size:S", "id": 6}] if rest.startswith("labels") else []
        if method == "POST":
            issues.append({**payload, "number": 9, "state": "open", "user": {"login": pr_review.BOT}})
            return issues[-1]
        if method == "PATCH":
            issues[-1].update(payload)
            return issues[-1]
        raise AssertionError(f"the heartbeat made a call it may not: {method} {path}")
    failing = [r for r in seen("queue.yml", when("2026-11-04T11:18:27Z"))]
    green = seen("queue.yml", when("2026-11-04T17:00:36Z"))
    state = {}
    with mock.patch.object(forge_pr, "fgj_api", answer), mock.patch.object(forge_pr, "repo", lambda: "o/r"), \
            contextlib.redirect_stdout(io.StringIO()):
        for records, at in ((failing, "2026-11-04T11:18:27Z"), (failing, "2026-11-04T11:23:27Z"), (green, "2026-11-04T17:00:36Z")):
            state["records"] = sorted(records, key=lambda r: r["created_at"], reverse=True)
            with mock.patch.object(time, "time", lambda: when(at)):
                heartbeat("queue.yml", "shadow")
        opened = [c for c in calls if c[0] == "POST"]
        state["records"] = failing
        before = len(calls)
        heartbeat("queue.yml", "off")
    if len(opened) != 1 or [c for c in calls if c[0] == "PATCH"] != [("PATCH", "repos/o/r/issues/9")] or issues[0]["state"] != "closed":
        bad.append(f"the heartbeat did not open once, leave it, then close it green: {calls}, {issues}")
    if len(calls) != before:
        bad.append("a heartbeat under QUEUE_ROWS off asked the forge")
    return bad


def offline_cases():
    """Every case with the forge's read failing: one that reaches it is a gate `just check` cannot run, since
    the guardrails lane installs no fgj."""
    cases = (rule_cases, git_cases, build_cases, record_cases, landing_cases, shadow_cases, forge_cases, queue_verb_cases,
             batch_cases, conflict_cases, green_cases, candidate_gate_cases, state_cases, report_cases, neutral_cases,
             tick_call_cases, parked_cases, rows_cases, consent_cases, heartbeat_cases)
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
    bad += [f"state `{name}` disabled and every case still reads its state, so it refutes nothing"
            for name, *_ in STATES if not state_cases(name)]
    for name, call in (("red batch", lambda: batch_cases("red batch")), ("member-only conflict", lambda: conflict_cases("main_conflict")),
                       ("green batch of four", lambda: green_cases("K")), ("landed batch recovery", lambda: green_cases("FORK"))):
        if not call():
            bad.append(f"{name} passes with its new behavior disabled, so its scenario refutes nothing")
    for act in ("enqueue", "promote", "repoint", "say"):
        with mock.patch.object(sys.modules[__name__], "STATES", tuple(row[:4] + (None if row[4] == act else row[4],) for row in STATES)):
            if not rows_cases():
                bad.append(f"the `{act}` act disabled and every rows case still passes, so it refutes nothing")
    for name, patch in (("the owner-only enqueue", mock.patch.object(sys.modules[__name__], "MERGERS", MERGERS | {"someone"})),
                        ("the switch on bot consent", mock.patch.object(sys.modules[__name__], "rows_mode", lambda ask=False: "on")),
                        ("the three-failure rule", mock.patch.object(sys.modules[__name__], "finished", lambda records: []))):
        with patch:
            if not (rows_cases() + consent_cases() + heartbeat_cases()):
                bad.append(f"{name} disabled and every case still passes, so it refutes nothing")
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
    mode, rows = os.environ.get("MERGE_QUEUE", "off"), rows_mode()
    heartbeat("autofix.yml", rows)
    if mode not in ("shadow", "on"):
        print(f"queue: MERGE_QUEUE is {mode}; nothing to do but the state report and the rows")
        # Invariant: an off queue takes no queue action, so the report reads the listing itself.
        if os.environ.get("STATE_REPORT") == "on" or rows != "off":
            report(listing(forge_pr.open_pulls(), "the open pull requests"), mode=rows)
        return 0
    return tick(mode)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
