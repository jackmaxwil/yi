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
    # Incident: a request that never answered held the live job's last step for the
    # runner's whole clock, three runs in a row; a hang is a failure with a traceback now.
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.load(response)


def upsert(transport, api, repo, pr, body):
    """PATCH the comment whose body starts with the marker, else POST a new one.

    Paged rather than one big GET: a busy PR pushes the marker off the first
    page, and a miss there does not fail — it silently posts a duplicate.
    """
    marker = body.splitlines()[0]
    issue = f"{api}/repos/{repo}/issues/{pr}/comments"
    page = 1
    while True:
        batch = transport("GET", f"{issue}?limit=50&page={page}", None)
        if not batch:
            return transport("POST", issue, {"body": body})
        for comment in batch:
            if (comment.get("body") or "").startswith(marker):
                url = f"{api}/repos/{repo}/issues/comments/{comment['id']}"
                return transport("PATCH", url, {"body": body})
        page += 1


def selfcheck():
    def recorder(pages):
        calls = []

        def transport(method, url, payload):
            calls.append((method, url, payload))
            if method != "GET":
                return {"id": 1}
            return pages.pop(0) if pages else []

        return transport, calls

    api, repo, pr = "https://git.example/api/v1", "apex/yi", 3
    marker = "Measured against `origin/main` by `just pr-body`"
    body = f"{marker} — the sections below are counted.\n\n## Files edited\n- x\n"

    # Nothing carries the marker — and a comment with a null body is a shape the
    # API really returns, not a hypothetical.
    transport, calls = recorder([[{"id": 7, "body": "nice"}, {"id": 8, "body": None}]])
    upsert(transport, api, repo, pr, body)
    assert calls[-1][0] == "POST", calls
    assert calls[-1][1] == f"{api}/repos/{repo}/issues/{pr}/comments", calls[-1][1]

    # The marker's comment is edited in place wherever the paging finds it.
    transport, calls = recorder([[{"id": 7, "body": "nice"}], [{"id": 9, "body": body}]])
    upsert(transport, api, repo, pr, body)
    assert calls[-1][0] == "PATCH", calls
    assert calls[-1][1] == f"{api}/repos/{repo}/issues/comments/9", calls[-1][1]

    # Idempotence is the point: a second push whose counts moved still edits
    # the first run's comment rather than stacking a changelog under the PR.
    # Which is also why the marker is the whole first line and that line is a
    # fixed echo in the workflow — reword it and every open PR orphans one.
    stale = body.replace("- x", "- y")
    transport, calls = recorder([[{"id": 9, "body": stale}]])
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
