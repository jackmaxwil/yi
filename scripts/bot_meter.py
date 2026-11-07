#!/usr/bin/env python3
"""What the review and autofix bots spend, read from the sessions their `yi ask` calls wrote, and
the status line every bot comment ends with. Each comment's meta line carries the numbers too, so
the comments are the ledger: a PR's total and a day's total are sums over them, and `just pr spend`
reads the same lines. No store of its own.
"""
import collections
import datetime
import json
import math
import os
import pathlib
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import forge_pr  # noqa: E402
import forge_tracking  # noqa: E402

BOT = "yi-bot"
_AUTHORS = None
# The meta lines whose `cost=` the totals sum: a review round, a voided round, an autofix attempt.
MARKERS = ("<!-- yi-round-meta ", "<!-- yi-round-void-meta ", "<!-- yi-autofix-meta ")
FIELDS = ("cost", "tin", "tout", "tcache", "calls", "turns", "secs", "models")


def skill_name(url):
    if url.startswith("yi://skills/"):
        address = url[len("yi://skills/"):]
        return address.split("/", 1)[0].split("#", 1)[0]
    if url.startswith("local://") and url.endswith("/SKILL.md"):
        return url[:-len("/SKILL.md")].rsplit("/", 1)[-1]
    return None


class Meter:
    """Usage summed over every `yi ask` call one comment's work made; safe across the threads a
    round's lenses and refuters run on."""

    def __init__(self):
        self.lock, self.started = threading.Lock(), time.monotonic()
        self.calls = self.turns = self.tin = self.tout = self.tcache = 0
        self.cost, self.models, self.skills = 0.0, set(), collections.Counter()

    def add_sessions(self, directory):
        """One call's session files: every assistant reply's usage, model and skill reads."""
        turns, tin, tout, tcache, cost, models = 0, 0, 0, 0, 0.0, set()
        skills = collections.Counter()
        for path in pathlib.Path(directory).rglob("*.jsonl"):
            for line in path.read_text(errors="replace").splitlines():
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if entry.get("customType") == "fetch":
                    name = skill_name(str((entry.get("data") or {}).get("url") or ""))
                    if name:
                        skills[name] += 1
                message = entry.get("message") or {}
                usage = message.get("usage") or {}
                if message.get("role") != "assistant" or not usage:
                    continue
                turns += 1
                tin += int(usage.get("input") or 0)
                tout += int(usage.get("output") or 0)
                tcache += int(usage.get("cacheRead") or 0)
                cost += float((usage.get("cost") or {}).get("total") or 0)
                if message.get("model"):
                    models.add(message["model"])
        with self.lock:
            self.calls += 1
            self.turns, self.tin, self.tout, self.tcache = self.turns + turns, self.tin + tin, self.tout + tout, self.tcache + tcache
            self.cost += cost
            self.models |= models
            self.skills += skills

    def fields(self):
        return {"cost": f"{self.cost:.4f}", "tin": self.tin, "tout": self.tout, "tcache": self.tcache,
                "calls": self.calls, "turns": self.turns, "secs": int(time.monotonic() - self.started),
                "models": ",".join(sorted(m.split("/")[-1] for m in self.models)) or "none",
                "skills": ",".join(f"{name}:{n}" for name, n in sorted(self.skills.items())) or "none"}


def meta(fields):
    return " ".join(f"{k}={fields[k]}" for k in FIELDS if k in fields)


def tokens(n):
    n = int(n)
    return f"{n / 1e6:.2f}M" if n >= 1e6 else f"{n / 1e3:.1f}k" if n >= 1e3 else str(n)


def duration(secs):
    secs = int(secs)
    return f"{secs // 60}m{secs % 60:02d}s" if secs >= 60 else f"{secs}s"


def status_line(meter, pr_total, day_total, what):
    """The last visible line of a bot comment: what this comment cost and where the totals stand."""
    f = meter.fields()
    return (f"<sub>{what} · {f['models'].replace(',', ', ')} · {f['calls']} call(s), {f['turns']} turn(s) · in {tokens(f['tin'])} "
            f"(cached {tokens(f['tcache'])}) · out {tokens(f['tout'])} · ${float(f['cost']):.2f} · "
            f"{duration(f['secs'])} · skills read {f['skills']} · this PR ${pr_total:.2f} · today ${day_total:.2f}</sub>")


def authors():
    """The logins a meta line is the ledger from: the bot's, and the owner's forge login, which
    `just pr retry` writes its gate rerun rows under. A commenter who is neither is not the ledger."""
    named = os.environ.get("YI_ROUND_AUTHORS")
    if named:
        return set(named.split(","))
    global _AUTHORS
    if _AUTHORS is None:
        me = forge_pr.fgj_api("GET", "user") or {}
        _AUTHORS = {BOT, me.get("login") if isinstance(me, dict) else None} - {None}
    return _AUTHORS


def rows(comments, trusted=None):
    """Every bot meta line in `comments`, as fields, with the comment's PR and time. A gate rerun row
    counts from a trusted login too, so the retry ledger is the owner's own; anyone else's is text."""
    out, allowed = [], trusted
    for comment in comments:
        login = (comment.get("user") or {}).get("login")
        bot = login == BOT
        for line in (comment.get("body") or "").splitlines()[:3]:
            for marker in MARKERS:
                if line.startswith(marker) and line.endswith(" -->"):
                    fields = dict(p.split("=", 1) for p in line[len(marker):-4].split() if "=" in p)
                    if not bot:
                        allowed = authors() if allowed is None else allowed
                        if fields.get("verdict") != "rerun" or login not in allowed:
                            continue
                    url = comment.get("pull_request_url") or comment.get("issue_url") or ""
                    out.append({**fields, "kind": marker[5:].split("-meta")[0], "at": comment.get("created_at", ""), "id": comment.get("id", 0),
                                "pr": fields.get("pr") or url.rsplit("/", 1)[-1]})
    return out


def cost(row):
    """A row's cost, or 0 for anything unreadable: a mangled line may not sink a whole spend pass."""
    try:
        value = float(row.get("cost") or 0)
    except (TypeError, ValueError):
        return 0.0
    return value if math.isfinite(value) else 0.0


def spent(found):
    return sum(cost(row) for row in found)


def since(repo, start):
    """Every comment on the repository since `start`."""
    transport = lambda method, url, body: forge_pr.fgj_api(method, url, body)
    return [c for c in forge_tracking.paged(transport, f"repos/{repo}/issues/comments?since={start}") if isinstance(c, dict)]


def midnight():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT00:00:00Z")


def totals(repo, pr_comments, meter):
    """(this PR's bot spend, today's bot spend), each including the comment about to be posted."""
    return spent(rows(pr_comments)) + meter.cost, spent(rows(since(repo, midnight()))) + meter.cost


def cmd_spend(args):
    """Bot spend per day, per PR and per kind, from the comments' own meta lines."""
    start = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=args.days)).strftime("%Y-%m-%dT00:00:00Z")
    found = rows(since(forge_pr.repo(), start))
    if not found:
        print(f"spend: no bot comment carries a cost since {start[:10]}")
        return 0
    for title, key in (("day", lambda r: r["at"][:10]), ("pr", lambda r: f"#{r['pr']}"), ("kind", lambda r: r["kind"]), ("models", lambda r: r.get("models") or "-")):
        groups = {}
        for row in found:
            groups.setdefault(key(row), []).append(row)
        print(f"{title:<12} {'cost':>9} {'comments':>9} {'in':>9} {'out':>8} {'cached':>9}")
        for name, group in sorted(groups.items()):
            sums = {k: sum(int(r.get(k) or 0) for r in group) for k in ("tin", "tout", "tcache")}
            print(f"{name:<12} {spent(group):>9.2f} {len(group):>9} {tokens(sums['tin']):>9} {tokens(sums['tout']):>8} {tokens(sums['tcache']):>9}")
        print()
    print(f"total ${spent(found):.2f} over {len(found)} costed comment(s) since {start[:10]}")
    return 0


def selfcheck():
    import tempfile, shutil
    errs = []
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="yi-meter-"))
    try:
        (tmp / "a").mkdir()
        reply = lambda i, o, c, cost, model: json.dumps({"message": {"role": "assistant", "model": model, "usage": {
            "input": i, "output": o, "cacheRead": c, "cost": {"total": cost}}}})
        (tmp / "a/s.jsonl").write_text("\n".join([json.dumps({"kind": "header"}), reply(1000, 50, 800, 0.01, "z-ai/glm-5.3-flash"),
                                                  json.dumps({"message": {"role": "user"}}), reply(2000, 70, 1900, 0.02, "z-ai/glm-5.3-flash"), "not json"]))
        meter = Meter()
        meter.add_sessions(tmp / "a")
        meter.add_sessions(tmp / "empty")
        f = meter.fields()
        if (f["calls"], f["turns"], f["tin"], f["tout"], f["tcache"], f["cost"], f["models"]) != (2, 2, 3000, 120, 2700, "0.0300", "glm-5.3-flash"):
            errs.append(f"the meter read {f}")
        if "models=glm-5.3-flash" not in meta(f).split():
            errs.append(f"the meta line drops the models a comment paid for: {meta(f)}")
        if f["skills"] != "none":
            errs.append(f"a session with no read counts a skill: {f['skills']}")
        read = lambda path: json.dumps({"message": {"role": "assistant", "content": [{"type": "toolCall", "name": "read", "arguments": {"path": path}}]}})
        fetch = lambda url: json.dumps({"type": "custom", "customType": "fetch", "data": {"url": url, "hash": "deadbeef", "servedBy": "skill"}})
        (tmp / "b").mkdir()
        (tmp / "b/s.jsonl").write_text("\n".join([read("yi://skills/ghost"), fetch("yi://skills/har-api"), fetch("local:///root/.yi/skills/har/SKILL.md")]))
        (tmp / "b/t.jsonl").write_text("\n".join([read("/root/.claude/skills/missing/SKILL.md"), fetch("local:///root/.claude/skills/har-io/SKILL.md"), fetch("yi://skills/har")]))
        reads = Meter()
        reads.add_sessions(tmp / "b")
        if reads.fields()["skills"] != "har:2,har-api:1,har-io:1":
            errs.append(f"the meter's served skill reads: {reads.fields()['skills']}")
        if "skills read har:2,har-api:1,har-io:1" not in status_line(reads, 0, 0, "round"):
            errs.append("the status line does not say which skills a round read")
        line = status_line(meter, 1.234, 5.5, "round 2")
        for part in ("round 2", "glm-5.3-flash", "2 call(s), 2 turn(s)", "in 3.0k", "(cached 2.7k)", "out 120", "$0.03", "this PR $1.23", "today $5.50"):
            if part not in line:
                errs.append(f"the status line lacks {part!r}: {line}")
    finally:
        shutil.rmtree(tmp)
    bot = {"user": {"login": BOT}, "created_at": "2026-10-01T10:00:00Z", "pull_request_url": "x/pulls/9"}
    found = rows([dict(bot, body="<!-- yi-round 1 -->\n<!-- yi-round-meta pr=9 sha=abc verdict=clean cost=0.2500 tin=10 -->\n"),
                  dict(bot, body="<!-- yi-autofix -->\n<!-- yi-autofix-meta pr=9 cost=0.5000 models=gpt-6.1-sol verdict=pushed -->\n"),
                  dict(bot, body="<!-- yi-round-void -->\n<!-- yi-round-void-meta pr=9 cost=0.1000 -->\n"),
                  {"user": {"login": "someone"}, "body": "<!-- yi-autofix-meta pr=9 cost=99 -->"},
                  dict(bot, body="prose that mentions <!-- yi-autofix-meta cost=7 --> later\n\n\n")])
    stranger = {"user": {"login": "someone"}, "body": "<!-- yi-autofix -->\n<!-- yi-autofix-meta pr=9 kind=gate from=abc verdict=rerun cost=99 -->"}
    owner = {"user": {"login": "jack"}, "body": "<!-- yi-autofix -->\n<!-- yi-autofix-meta pr=9 kind=gate from=abc verdict=rerun -->"}
    saved = os.environ.get("YI_ROUND_AUTHORS")
    os.environ["YI_ROUND_AUTHORS"] = "jack"
    try:
        if rows([stranger]) != []:
            errs.append("a stranger's rerun row is the ledger: any login that can comment retires the head's retries")
        if [r["verdict"] for r in rows([owner])] != ["rerun"]:
            errs.append("the owner's own rerun row is the retry ledger and was dropped")
    finally:
        if saved is None:
            os.environ.pop("YI_ROUND_AUTHORS", None)
        else:
            os.environ["YI_ROUND_AUTHORS"] = saved
    try:
        mangled = cost({"cost": "cheap"})
    except (TypeError, ValueError):
        errs.append("a mangled cost raises out of a spend total instead of counting nothing")
    else:
        if mangled != 0.0:
            errs.append(f"a mangled cost counted as {mangled}")
    if [r.get("models") for r in found] != [None, "gpt-6.1-sol", None]:
        errs.append("the models a comment's cost went to are not read back")
    if round(spent(found), 4) != 0.85 or [r["kind"] for r in found] != ["yi-round", "yi-autofix", "yi-round-void"]:
        errs.append(f"the ledger read {found}: only the bot's own meta lines, at the top, count")
    if tokens(1_234_567) != "1.23M" or duration(492) != "8m12s":
        errs.append("the units misprint")
    if errs:
        print("FAIL bot_meter selfcheck")
        for err in errs:
            print(f"  {err}")
        return 1
    print("ok   bot_meter selfcheck")
    return 0


if __name__ == "__main__":
    sys.exit(selfcheck() if "--selfcheck" in sys.argv[1:] else (print("use `just pr spend`", file=sys.stderr) or 2))
