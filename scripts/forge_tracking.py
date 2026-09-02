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
import subprocess
import sys

from forgejo_pr_comment import send

# docs/TODOS.md's own sizing, which the labels carry: S = a day, M = three, L = a week.
WEIGHTS = {"size:S": 1, "size:M": 3, "size:L": 7}


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


def selfcheck():
    def recorder(pages):
        calls = []

        def transport(method, url, payload):
            calls.append((method, url, payload))
            if method != "GET":
                return {}
            for key, batches in pages.items():
                if key in url:
                    return batches.pop(0) if batches else []
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
    print("ok   forge_tracking selfcheck")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--measure", action="store_true")
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
    if not (args.measure and args.repo and args.api):
        parser.print_usage(sys.stderr)
        return 2
    today = datetime.datetime.now(datetime.timezone.utc).date()
    commit = subprocess.run(
        ("git", "rev-parse", "--short", "HEAD"), capture_output=True, text=True, check=False
    ).stdout.strip()
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
