#!/usr/bin/env python3
"""Derive milestone due dates from the forge's own issue history.

The forge is the register, so a date on it is measured from what it already
holds rather than from a table in the tree that drifts the day after it is
written: size-days closed in a trailing window give a rate, and a milestone is
due when its open size-days divide out. Transport is a parameter, which is what
lets the selfcheck exercise the arithmetic and the routing without a server.
"""
import argparse
import datetime
import math
import os
import pathlib
import re
import subprocess
import sys

from forgejo_pr_comment import send, upsert

# The sizing the register was written in and the labels carry: S = a day, M = three, L = a week.
WEIGHTS = {"size:S": 1, "size:M": 3, "size:L": 7}
ROOT = pathlib.Path(__file__).resolve().parents[1]
# Forgejo's own default keywords, because they are what closed the issue: a
# wider pattern would annotate issues this merge only mentioned.
CLOSES = re.compile(r"\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)", re.I)
PULL = re.compile(r"\(#(\d+)\)")
# The estate timer's contract (the infra repository scripts/forgejo_tracking_check.py,
# verdict_body()) -- the pinned Tracking issue's first line is exactly:
#     verdict: green|red · <ISO timestamp> · <n> cards checked
# joined with " · " (middle dot, U+00B7), stamped by
# datetime.now(UTC).replace(microsecond=0).isoformat(). Anything else -- red,
# truncated, foreign -- is a miss; this must not degenerate into a substring
# search for "green", since a red verdict's own miss lines can contain that word.
VERDICT_RE = re.compile(r"^verdict: (green|red) · \S+ · \d+ cards checked$")


def git(*args):
    """Trailing whitespace off, because every caller of this is building a string."""
    return subprocess.run(("git", *args), capture_output=True, text=True, check=False).stdout.strip()


def version():
    """The version in docs/ARCHITECTURE.md's header — the one the merge shipped."""
    header = (ROOT / "docs/ARCHITECTURE.md").read_text()
    return re.search(r"^version:\s*(\S+)", header, re.M).group(1)


def points(issue):
    return sum(WEIGHTS.get(label["name"], 0) for label in issue["labels"])


def paged(transport, url):
    """Every list endpoint here pages, and a miss on page 1 is silent, not an error."""
    page = 1
    while True:
        batch = transport("GET", f"{url}&limit=50&page={page}", None)
        if not batch:
            return
        yield from batch
        page += 1


def measure(transport, api, repo, window, today):
    """(closed size-days, rows) where a row is (milestone, open days, days, due).

    Days are integer arithmetic on the rate's own fraction — open * window /
    closed — because hygiene recomputes this and compares, so a float that
    rounds differently on another host would be a false red.
    """
    issues = f"{api}/repos/{repo}/issues?type=issues"
    cutoff = today - datetime.timedelta(days=window)
    closed = 0
    for issue in paged(transport, f"{issues}&state=closed"):
        at = issue.get("closed_at")
        if at and datetime.datetime.fromisoformat(at).date() >= cutoff:
            closed += points(issue)
    if not closed:
        raise SystemExit(f"FAIL {repo}: nothing closed in the last {window} days to measure")
    open_days = {}
    for issue in paged(transport, f"{issues}&state=open"):
        milestone = issue.get("milestone")
        if milestone:
            open_days[milestone["id"]] = open_days.get(milestone["id"], 0) + points(issue)
    rows = []
    for milestone in paged(transport, f"{api}/repos/{repo}/milestones?state=open"):
        days = open_days.get(milestone["id"], 0)
        if not days:
            continue
        rows.append((milestone, days, math.ceil(days * window / closed)))
    return closed, [
        (m, d, n, today + datetime.timedelta(days=n)) for m, d, n in sorted(rows, key=lambda r: r[2])
    ]


def describe(closed, window, today, commit, days, span, due):
    return (
        "Derived by `scripts/forge_tracking.py --measure` from this repository's closed"
        " issues. The weekly tracking-hygiene job recomputes it and fails on a mismatch,"
        " so a hand-edited date does not survive the week.\n\n"
        f"throughput: {closed} size-days closed in the {window} days to {today}"
        " (S=1, M=3, L=7)\n"
        f"open: {days} size-days\n"
        f"formula: due = {today} + ceil({days} * {window} / {closed}) = {today} + {span} days"
        f" = {due}\n"
        f"measured at: {commit}"
    )


def write_rows(transport, api, repo, rows, closed, window, today, commit):
    for milestone, days, span, due in rows:
        transport(
            "PATCH",
            f"{api}/repos/{repo}/milestones/{milestone['id']}",
            {
                "due_on": f"{due}T23:59:59Z",
                "description": describe(closed, window, today, commit, days, span, due),
            },
        )


def closed_by(transport, api, repo, message):
    """The issues a merge closed, from the PR it names and from its own message.

    A merge subject carries the pull request's number and the close keywords sit
    in that pull request's body; a squash or a direct push carries them here.
    """
    numbers = set(CLOSES.findall(message))
    named = PULL.search(message.splitlines()[0] if message else "")
    if named:
        pull = transport("GET", f"{api}/repos/{repo}/pulls/{named.group(1)}", None)
        numbers.update(CLOSES.findall(pull.get("body") or ""))
    return sorted(int(number) for number in numbers)


def comment_closed(transport, api, repo, message, release, sha):
    """One comment per issue, naming the version and the merge that closed it.

    The body is one line and the whole line is the marker, so a re-run of the
    same postmerge job edits its own comment instead of stacking a second.
    """
    numbers = closed_by(transport, api, repo, message)
    for number in numbers:
        upsert(transport, api, repo, number, f"Closed by yi {release}, merge {sha[:12]}.")
    return numbers


def hygiene(transport, api, repo, window, today):
    """(misses, notes). A miss is what is wrong and the command that fixes it."""
    misses, notes, pinned = [], [], None
    for issue in paged(transport, f"{api}/repos/{repo}/issues?type=issues&state=open"):
        number = issue["number"]
        names = [label["name"] for label in issue["labels"]]
        sizes = [name for name in names if name in WEIGHTS]
        if issue.get("pin_order") and issue["title"] == "Tracking":
            pinned = issue
            # Infrastructure the estate timer writes, not planned work: sizing
            # or milestoning it would fold its size into the milestone's own
            # due-date arithmetic below.
            continue
        if len(sizes) > 1:
            misses.append(
                (
                    f"#{number} carries {len(sizes)} size labels ({', '.join(sizes)})",
                    f"fgj issue edit {number} {' '.join('--remove-label ' + s for s in sizes[1:])} -R {repo}",
                )
            )
        elif not sizes:
            misses.append(
                (
                    f"#{number} has no size label",
                    f"fgj issue edit {number} --add-label size:M -R {repo}",
                )
            )
        if not [name for name in names if name.startswith("area:")]:
            misses.append(
                (
                    f"#{number} has no area label",
                    f"fgj issue edit {number} --add-label area:runtime -R {repo}",
                )
            )
        if not issue.get("milestone"):
            misses.append(
                (
                    f"#{number} has no milestone",
                    f"fgj api -X PATCH repos/{repo}/issues/{number} -F milestone=<id>  # ids: fgj api repos/{repo}/milestones",
                )
            )
    closed, rows = measure(transport, api, repo, window, today)
    for milestone, days, _span, due in rows:
        title, have = milestone["title"], (milestone.get("due_on") or "")[:10]
        if have != str(due):
            misses.append(
                (
                    f"milestone {title!r} is due {have or 'never'}; {days} open size-days at"
                    f" {closed} closed in {window} days derives {due}",
                    f"python3 scripts/forge_tracking.py --measure --repo {repo}",
                )
            )
        if have and have < str(today):
            misses.append(
                (
                    f"milestone {title!r} is due {have}, in the past, with {days} size-days open",
                    f"python3 scripts/forge_tracking.py --measure --repo {repo}",
                )
            )
    if pinned is None:
        # A warning until the estate timer that writes it lands; a repo with no
        # tracking verdict is unproven, not yet broken.
        notes.append("no pinned issue titled Tracking; the estate timer has not landed")
        return misses, notes
    number = pinned["number"]
    read = f"fgj issue view {number} -R {repo}; fgj api repos/{repo}/issues/{number}/comments"
    first = ((pinned.get("body") or "").strip().splitlines() or [""])[0]
    match = VERDICT_RE.match(first)
    if not match or match.group(1) != "green":
        misses.append((f"pinned Tracking #{number} does not open green: {first[:60]!r}", read))
    age = (today - datetime.datetime.fromisoformat(pinned["updated_at"]).date()).days
    if age > 8:
        misses.append((f"pinned Tracking #{number} was last written {age} days ago", read))
    return misses, notes


def selfcheck():
    def recorder(pages):
        calls = []

        def transport(method, url, payload):
            calls.append((method, url, payload))
            if method != "GET":
                return {}
            page = int(url.split("page=")[1]) if "page=" in url else 1
            for key, batches in pages.items():
                if key in url:
                    return batches[page - 1] if page <= len(batches) else []
            return []

        return transport, calls

    def issue(number, size, closed_at=None, milestone=None):
        return {
            "number": number,
            "labels": [{"name": size}, {"name": "area:runtime"}],
            "closed_at": closed_at,
            "milestone": milestone,
        }

    today = datetime.date(2026, 3, 10)
    pages = {
        # Two pages, because a closed issue on page 2 is size-days the rate must
        # still count; the third is outside the window and must not be counted.
        "issues?type=issues&state=closed": [
            [issue(1, "size:M", "2026-03-09T00:00:00Z")],
            [issue(2, "size:S", "2026-03-01T00:00:00Z"), issue(3, "size:L", "2026-01-01T00:00:00Z")],
        ],
        "issues?type=issues&state=open": [
            [
                issue(10, "size:L", milestone={"id": 1}),
                issue(11, "size:M", milestone={"id": 1}),
                issue(12, "size:S", milestone={"id": 2}),
            ]
        ],
        "milestones?": [
            [
                {"id": 1, "title": "wide", "due_on": None},
                {"id": 2, "title": "narrow", "due_on": None},
                {"id": 3, "title": "empty", "due_on": "2026-01-01T23:59:59Z"},
            ]
        ],
    }
    transport, calls = recorder(pages)
    closed, rows = measure(transport, "https://git.example/api/v1", "apex/yi", 14, today)
    assert closed == 4, closed
    assert [(m["title"], d, n, str(due)) for m, d, n, due in rows] == [
        ("narrow", 1, 4, "2026-03-14"),
        ("wide", 10, 35, "2026-04-14"),
    ], rows

    transport, calls = recorder({})
    write_rows(transport, "https://git.example/api/v1", "apex/yi", rows, 4, 14, today, "abc1234")
    assert [c[0] for c in calls] == ["PATCH", "PATCH"], calls
    assert calls[1][1].endswith("/milestones/1"), calls[1][1]
    assert calls[0][2]["due_on"] == "2026-03-14T23:59:59Z", calls[0][2]
    body = calls[1][2]["description"]
    for fragment in ("4 size-days closed in the 14 days to 2026-03-10", "abc1234", "= 2026-04-14"):
        assert fragment in body, body

    # A milestone with no open issues is left alone rather than dated at today.
    assert all("/milestones/3" not in c[1] for c in calls), calls
    # A merge closes what its pull request's body says, and what its own message
    # says — a squash carries the keywords in neither place twice.
    transport, calls = recorder({"/pulls/9": [{"body": "narrative\n\nCloses #40, fixes #41"}]})
    merge = "Merge pull request 'Do the thing' (#9) from feat/x into main\n\nResolved #42"
    assert closed_by(transport, "https://git.example/api/v1", "apex/yi", merge) == [40, 41, 42]

    # One comment per issue, and the marker is the line itself, so the second run
    # of the same postmerge job edits rather than stacks.
    transport, calls = recorder({"/comments": [[]]})
    annotated = comment_closed(
        transport, "https://git.example/api/v1", "apex/yi", "Fixes #7", "0.9.0", "a" * 40
    )
    assert annotated == [7], annotated
    assert [c[0] for c in calls] == ["GET", "POST"], calls
    assert calls[-1][2] == {"body": "Closed by yi 0.9.0, merge aaaaaaaaaaaa."}, calls[-1][2]

    # Hygiene names the issue and the command that fixes it. The pinned verdict is
    # absent here, which is a note rather than a miss until the timer lands.
    pages["issues?type=issues&state=open"] = [
        [
            issue(20, "size:L", milestone={"id": 1}),
            {"number": 21, "labels": [{"name": "size:S"}, {"name": "size:M"}], "milestone": None},
        ]
    ]
    pages["issues?type=issues&state=closed"] = [[issue(1, "size:M", "2026-03-09T00:00:00Z")]]
    pages["milestones?"] = [[{"id": 1, "title": "wide", "due_on": "2026-01-01T23:59:59Z"}]]
    transport, calls = recorder(pages)
    misses, notes = hygiene(transport, "https://git.example/api/v1", "apex/yi", 14, today)
    reported = [what for what, _fix in misses]
    assert any("#21 carries 2 size labels" in r for r in reported), reported
    assert any("#21 has no area label" in r for r in reported), reported
    assert any("#21 has no milestone" in r for r in reported), reported
    assert any("is due 2026-01-01, in the past" in r for r in reported), reported
    assert any("derives 2026-04-12" in r for r in reported), reported
    assert not [r for r in reported if r.startswith("#20")], reported
    assert notes == ["no pinned issue titled Tracking; the estate timer has not landed"], notes
    assert misses[1][1] == "fgj issue edit 21 --add-label area:runtime -R apex/yi", misses

    # The pinned verdict is read against the estate timer's real contract
    # (the infra repository scripts/forgejo_tracking_check.py, verdict_body()):
    #     verdict: green|red · <ISO timestamp> · <n> cards checked
    # A loose `startswith("green")` never matches this line -- it starts with
    # "verdict:" -- so a genuinely green verdict missed forever; that is the
    # bug this fixture is red-first proof against. Isolated from the
    # milestone-derived misses above: one closed issue satisfies measure()'s
    # "nothing closed" guard, and no open milestones means no due-date noise.
    pages["issues?type=issues&state=closed"] = [[issue(1, "size:M", "2026-03-09T00:00:00Z")]]
    pages["milestones?"] = [[]]

    def pinned_issue_pages(body, updated_at):
        # Infrastructure the estate timer writes, not planned work: it carries
        # no size, area or milestone by design, so those checks must not fire
        # on it -- a size or milestone would count it toward the milestone's
        # own due-date arithmetic, which is wrong.
        return [
            [
                {
                    "number": 20,
                    "title": "Tracking",
                    "pin_order": 1,
                    "labels": [{"name": "kind:decision"}],
                    "milestone": None,
                    "body": body,
                    "updated_at": updated_at,
                }
            ]
        ]

    fresh = today.isoformat() + "T00:00:00Z"
    real_green = (
        "verdict: green · 2026-03-09T12:00:00+00:00 · 42 cards checked\n\n"
        "every card is in the column its issue's state requires.\n"
    )
    real_red = (
        "verdict: red · 2026-03-09T12:00:00+00:00 · 42 cards checked\n\n"
        "#5 · in Backlog · must be in Next · in the earliest-due open milestone\n"
    )

    # A real green verdict is zero misses.
    pages["issues?type=issues&state=open"] = pinned_issue_pages(real_green, fresh)
    misses, notes = hygiene(recorder(pages)[0], "https://git.example/api/v1", "apex/yi", 14, today)
    assert misses == [], misses
    assert notes == [], notes

    # A real red verdict is exactly one miss.
    pages["issues?type=issues&state=open"] = pinned_issue_pages(real_red, fresh)
    misses, notes = hygiene(recorder(pages)[0], "https://git.example/api/v1", "apex/yi", 14, today)
    assert len(misses) == 1, misses
    assert "does not open green" in misses[0][0], misses

    # A red verdict's own miss lines can name a column called "green" -- a
    # substring search for "green" would wrongly pass this; it must still miss.
    tricky_red = (
        "verdict: red · 2026-03-09T12:00:00+00:00 · 42 cards checked\n\n"
        "#5 · in green-ish-backlog · must be in Next · rule text\n"
    )
    pages["issues?type=issues&state=open"] = pinned_issue_pages(tricky_red, fresh)
    misses, notes = hygiene(recorder(pages)[0], "https://git.example/api/v1", "apex/yi", 14, today)
    assert len(misses) == 1, misses
    assert "does not open green" in misses[0][0], misses

    # Truncated, empty and foreign first lines are each a miss, not a pass.
    truncated = real_green.splitlines()[0][:-3]  # a write cut off mid-word
    for bad_body in (truncated, "", "not a verdict line"):
        pages["issues?type=issues&state=open"] = pinned_issue_pages(bad_body, fresh)
        misses, notes = hygiene(recorder(pages)[0], "https://git.example/api/v1", "apex/yi", 14, today)
        assert len(misses) == 1, (bad_body, misses)
        assert "does not open green" in misses[0][0], misses

    # Staleness still fires on a genuinely green verdict written too long ago.
    pages["issues?type=issues&state=open"] = pinned_issue_pages(real_green, "2026-02-01T00:00:00Z")
    misses, notes = hygiene(recorder(pages)[0], "https://git.example/api/v1", "apex/yi", 14, today)
    assert len(misses) == 1, misses
    assert "last written 37 days ago" in misses[0][0], misses

    print("ok   forge_tracking selfcheck")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--measure", action="store_true")
    parser.add_argument("--hygiene", action="store_true")
    parser.add_argument("--comment-closed", action="store_true")
    parser.add_argument("--sha", default=os.environ.get("GITHUB_SHA"))
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--window", type=int, default=14)
    parser.add_argument("--repo", default=os.environ.get("FORGEJO_REPOSITORY"))
    parser.add_argument("--api", default=os.environ.get("FORGEJO_API_URL"))
    args = parser.parse_args(argv)
    if args.selfcheck:
        selfcheck()
        return 0
    if not os.environ.get("FORGEJO_TOKEN"):
        print("FORGEJO_TOKEN is unset; refusing to run", file=sys.stderr)
        return 2
    if not (args.measure or args.hygiene or args.comment_closed) or not (args.repo and args.api):
        parser.print_usage(sys.stderr)
        return 2
    today = datetime.datetime.now(datetime.timezone.utc).date()
    if args.comment_closed:
        sha = args.sha or git("rev-parse", "HEAD")
        numbers = comment_closed(
            send, args.api, args.repo, git("log", "-1", "--format=%B", sha), version(), sha
        )
        print(f"{args.repo}: {sha[:12]} closed {numbers or 'nothing'}")
        return 0
    if args.hygiene:
        misses, notes = hygiene(send, args.api, args.repo, args.window, today)
        for note in notes:
            print(f"warn {args.repo}: {note}")
        for what, fix in misses:
            print(f"MISS {what}\n     {fix}")
        print(f"{args.repo}: {len(misses)} misses")
        return 1 if misses else 0
    commit = git("rev-parse", "--short", "HEAD")
    closed, rows = measure(send, args.api, args.repo, args.window, today)
    rate = closed / args.window
    print(f"{args.repo}: {closed} size-days closed in {args.window} days = {rate:.2f}/day at {commit}")
    print(f"{'milestone':36} {'open':>4} {'days':>4}  {'due':10}  was")
    for milestone, days, span, due in rows:
        was = (milestone.get("due_on") or "-")[:10]
        print(f"{milestone['title']:36.36} {days:4} {span:4}  {due}  {was}")
    if args.dry_run:
        print(f"dry run: {len(rows)} milestones left untouched")
        return 0
    write_rows(send, args.api, args.repo, rows, closed, args.window, today, commit)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
