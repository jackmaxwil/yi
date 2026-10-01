#!/usr/bin/env python3
"""What the review and autofix bots spend, read from the sessions their `yi ask` calls wrote, and
the status line every bot comment ends with. Each comment's meta line carries the numbers too, so
the comments are the ledger: a PR's total and a day's total are sums over them, and `just pr spend`
reads the same lines. No store of its own.
"""
import datetime
import json
import pathlib
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import forge_pr  # noqa: E402

BOT = "yi-bot"
# The meta lines whose `cost=` the totals sum: a review round, a voided round, an autofix attempt.
MARKERS = ("<!-- yi-round-meta ", "<!-- yi-round-void-meta ", "<!-- yi-autofix-meta ")
FIELDS = ("cost", "tin", "tout", "tcache", "calls", "turns", "secs", "models")


class Meter:
    """Usage summed over every `yi ask` call one comment's work made; safe across the threads a
    round's lenses and refuters run on."""

    def __init__(self):
        self.lock, self.started = threading.Lock(), time.monotonic()
        self.calls = self.turns = self.tin = self.tout = self.tcache = 0
        self.cost, self.models = 0.0, set()

    def add_sessions(self, directory):
        """One call's session files: every assistant reply's usage and model."""
        turns, tin, tout, tcache, cost, models = 0, 0, 0, 0, 0.0, set()
        for path in pathlib.Path(directory).rglob("*.jsonl"):
            for line in path.read_text(errors="replace").splitlines():
                try:
                    message = json.loads(line).get("message") or {}
                except (json.JSONDecodeError, AttributeError):
                    continue
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

    def fields(self):
        return {"cost": f"{self.cost:.4f}", "tin": self.tin, "tout": self.tout, "tcache": self.tcache,
                "calls": self.calls, "turns": self.turns, "secs": int(time.monotonic() - self.started),
                "models": ",".join(sorted(m.split("/")[-1] for m in self.models)) or "none"}


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
            f"{duration(f['secs'])} · this PR ${pr_total:.2f} · today ${day_total:.2f}</sub>")


def rows(comments):
    """Every bot meta line in `comments`, as fields, with the comment's PR and time."""
    out = []
    for comment in comments:
        if (comment.get("user") or {}).get("login") != BOT:
            continue
        for line in (comment.get("body") or "").splitlines()[:3]:
            for marker in MARKERS:
                if line.startswith(marker) and line.endswith(" -->"):
                    fields = dict(p.split("=", 1) for p in line[len(marker):-4].split() if "=" in p)
                    url = comment.get("pull_request_url") or comment.get("issue_url") or ""
                    out.append({**fields, "kind": marker[5:].split("-meta")[0], "at": comment.get("created_at", ""),
                                "pr": fields.get("pr") or url.rsplit("/", 1)[-1]})
    return out


def spent(found):
    return sum(float(row.get("cost") or 0) for row in found)


def since(repo, start):
    """Every comment on the repository since `start`, paged until a page adds nothing new."""
    out, seen = [], set()
    for page in range(1, 41):
        batch = forge_pr.fgj_api("GET", f"repos/{repo}/issues/comments?since={start}&limit=50&page={page}") or []
        fresh = [c for c in batch if isinstance(c, dict) and c.get("id") not in seen]
        if not fresh:
            break
        seen |= {c.get("id") for c in fresh}
        out += fresh
    return out


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
