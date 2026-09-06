#!/usr/bin/env python3
"""Upsert one PR comment on Forgejo, keyed by the body's own first line.

Forgejo ships no `gh`, so `gh pr comment --edit-last` has no port; this is the
same edit-in-place over the REST API. The marker is the body's first line
rather than a constant, so the workflow does not hold a second copy of it.
Transport is a parameter, which is what lets the selfcheck exercise the routing
without a server.
"""
import json
import os
import pathlib
import sys
import urllib.request


def send(method, url, payload):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={
            "Authorization": f"token {os.environ['FORGEJO_TOKEN']}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.load(response)


def upsert(transport, api, repo, pr, body):
    """PATCH the comment whose body starts with the marker, else POST a new one.

    Incident: Forgejo 15 answers one issue's comments as the whole list and ignores
    `page`, so paging until an empty batch looped for the runner's whole clock on
    every PR that had comments but no marker yet — the live job, twice.
    """
    marker = body.splitlines()[0]
    issue = f"{api}/repos/{repo}/issues/{pr}/comments"
    for comment in transport("GET", issue, None):
        if (comment.get("body") or "").startswith(marker):
            url = f"{api}/repos/{repo}/issues/comments/{comment['id']}"
            return transport("PATCH", url, {"body": body})
    return transport("POST", issue, {"body": body})


def selfcheck():
    def recorder(comments):
        calls = []

        def transport(method, url, payload):
            calls.append((method, url, payload))
            return comments if method == "GET" else {"id": 1}

        return transport, calls

    api, repo, pr = "https://git.example/api/v1", "apex/yi", 3
    marker = "Measured against `origin/main` by `just pr-body`"
    body = f"{marker} — the sections below are counted.\n\n## Files edited\n- x\n"

    # Nothing carries the marker — and a comment with a null body is a shape the
    # API really returns, not a hypothetical.
    transport, calls = recorder([{"id": 7, "body": "nice"}, {"id": 8, "body": None}])
    upsert(transport, api, repo, pr, body)
    assert calls[-1][0] == "POST", calls
    assert calls[-1][1] == f"{api}/repos/{repo}/issues/{pr}/comments", calls[-1][1]

    # The marker's comment is edited in place wherever it sits in the list.
    transport, calls = recorder([{"id": 7, "body": "nice"}, {"id": 9, "body": body}])
    upsert(transport, api, repo, pr, body)
    assert calls[-1][0] == "PATCH", calls
    assert calls[-1][1] == f"{api}/repos/{repo}/issues/comments/9", calls[-1][1]

    # Idempotence is the point: a second push whose counts moved still edits
    # the first run's comment rather than stacking a changelog under the PR.
    # Which is also why the marker is the whole first line and that line is a
    # fixed echo in the workflow — reword it and every open PR orphans one.
    stale = body.replace("- x", "- y")
    transport, calls = recorder([{"id": 9, "body": stale}])
    upsert(transport, api, repo, pr, body)
    assert [c[0] for c in calls] == ["GET", "PATCH"], calls
    assert calls[-1][2] == {"body": body}, calls[-1][2]
    print("ok   forgejo_pr_comment selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    if len(argv) != 1:
        print("usage: forgejo_pr_comment.py <body-file>", file=sys.stderr)
        return 2
    upsert(
        send,
        os.environ["FORGEJO_API_URL"],
        os.environ["FORGEJO_REPOSITORY"],
        os.environ["FORGEJO_PR"],
        pathlib.Path(argv[0]).read_text(),
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
