#!/usr/bin/env python3
"""PR title and body, judged from CI where the forge is reachable.

Title: the same rule the commit subjects are judged by — `subject_errors` from
check_commit_style, imported rather than restated. It exists nowhere in the tree
the local hook scans, so only CI can see it.

Body: a change that adds a feature-ledger row or grows src past the free band is
work, and work has an issue (D106). Such a body names it — `Closes #N` finishing
it, `Refs #N` as part of it — the issue is open, sized once, has an area and a
milestone, and a changelog row added by the same diff cites the same number.
Everything else passes silently: a ratchet, a doc fix and an in-band repair are
exempt by construction, not by an author's say-so.

This gate never runs offline (040). It asks the forge questions a machine
without one cannot answer, and a gate that guesses is worse than no gate.
"""
import json
import os
import pathlib
import re
import sys
import urllib.error
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from _common import ROOT, FREE_BAND, fail  # noqa: E402
from check_commit_style import NAMED, subject_errors  # noqa: E402
from pr_body import base_commit, diff_stats, git, tally  # noqa: E402

ARCH = "docs/ARCHITECTURE.md"
LOG = "docs/CHANGELOG.md"
LEDGER = "Feature ledger"
CHANGELOG = "Changelog"
# `Closes apex/yi#4` is the same citation as `Closes #4`; `Closes other/repo#4`
# is a citation of someone else's register and does not count as one here.
# Incident: every PR body for a week ended in an assistant's generated-with footer, the
# body-side twin of the co-author trailer commit_style already refuses.
FOOTER = re.compile(r"generated (with|by)|co-authored-by", re.I)
CITE = re.compile(r"\b(closes|refs)\s+(?:([\w.-]+/[\w.-]+))?#(\d+)\b", re.I)


def send(method, url, payload=None):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={
            "Authorization": f"token {os.environ['FORGEJO_TOKEN']}",
            "Accept": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(req) as response:
            return json.load(response)
    except urllib.error.HTTPError as err:
        if err.code == 404:
            return None
        raise


def table_rows(text, heading):
    """Data rows of the table under a heading, at whatever level it is written —
    the changelog is its own file's title, the ledger a section of another. Rows
    start after the `|---|` separator so the header row is not mistaken for a
    feature named "feature"; a section with no table yields nothing."""
    out, inside, started = [], False, False
    for line in text.splitlines():
        if line.startswith("#"):
            inside, started = line.strip().lstrip("#").strip() == heading, False
            continue
        line = line.strip()
        if not inside or not line.startswith("|"):
            continue
        if not set(line) - set("|-: "):
            started = True
        elif started:
            out.append(line)
    return out


def row_key(row):
    """A table row is identified by its first cell: the feature, or the version."""
    return row.split("|")[1].strip()


def added_rows(before, after, heading):
    """Rows whose key is new. Set difference, not a diff parse: a row that moved
    or was reworded in place is not a new row, and the diff would call it one."""
    was = {row_key(row) for row in table_rows(before, heading)}
    return [row for row in table_rows(after, heading) if row_key(row) not in was]


def cited(repo, body):
    """The issue numbers this body claims, in order, deduplicated."""
    out = []
    for _, named, number in CITE.findall(body or ""):
        if named and named != repo:
            continue
        if int(number) not in out:
            out.append(int(number))
    return out


def issue_problems(transport, api, repo, number):
    issue = transport("GET", f"{api}/repos/{repo}/issues/{number}")
    if not issue:
        return [
            f"#{number} is not an issue on {repo}",
            "  fgj issue list          # cite one that exists, or open it",
        ]
    errs = []
    if issue.get("state") != "open":
        errs += [
            f"#{number} is {issue.get('state')}; a merge cannot close it again",
            f"  fgj issue reopen {number}   # or cite the issue this work is really under",
        ]
    labels = [label.get("name", "") for label in issue.get("labels") or []]
    sizes = sorted(name for name in labels if name.startswith("size:"))
    areas = [name for name in labels if name.startswith("area:")]
    if not sizes:
        errs += [
            f"#{number} carries no `size:` label; exactly one is mandatory",
            f"  fgj issue edit {number} --add-label size:M",
        ]
    elif len(sizes) > 1:
        errs += [
            f"#{number} carries {len(sizes)} size labels ({', '.join(sizes)}); exactly one is",
            f"  fgj issue edit {number} {' '.join('--remove-label ' + s for s in sizes[1:])}",
        ]
    if not areas:
        errs += [
            f"#{number} carries no `area:` label",
            f"  fgj issue edit {number} --add-label area:runtime",
        ]
    if not issue.get("milestone"):
        errs += [
            f"#{number} has no milestone, and a milestone is what a due date is computed over",
            f"  fgj api --hostname git.example.invalid -X PATCH repos/{repo}/issues/{number} -F milestone=<id>",
        ]
    return errs


def footer_problems(body):
    """A line that credits an assistant as author or generator is a trailer, not a body."""
    return [
        f"the PR body credits an assistant: {line.strip()!r} — delete the line"
        for line in (body or "").splitlines()
        if FOOTER.search(line) and NAMED.search(line)
    ]


def body_problems(transport, api, repo, body, ledger_added, changelog_added, src_net):
    """The whole body rule, pure but for `transport`."""
    reasons = []
    if ledger_added:
        reasons.append("it adds the feature-ledger row " + ", ".join(f"`{k}`" for k in ledger_added))
    if src_net > FREE_BAND:
        reasons.append(f"its net src growth is {src_net:+d}, past the free band of +{FREE_BAND}")
    if not reasons:
        return []

    numbers = cited(repo, body)
    if not numbers:
        return [
            "the PR body names no issue, and " + "; ".join(reasons),
            "  add `Closes #N` (this PR finishes it) or `Refs #N` (this PR is part of it)",
            "  fgj issue list          # it is probably already open",
            '  fgj issue create -t "<imperative sentence>" -l size:M -l area:runtime',
            "  fgj api --hostname git.example.invalid -X PATCH repos/<owner/repo>/issues/<n> -F milestone=<id>",
        ]
    errs = []
    for number in numbers:
        errs += issue_problems(transport, api, repo, number)
    for row in changelog_added:
        if not any(f"#{number}" in row for number in numbers):
            errs += [
                f"the {row_key(row)} changelog row cites no issue; the body names "
                + ", ".join(f"#{n}" for n in numbers),
                f"  put the same `#N` in that row of {ARCH} — the row and the issue",
                "  have to name each other or neither is findable from the other",
            ]
    return errs


def selfcheck():
    """Every branch, against a transport that never leaves the process."""

    def forge(issues):
        seen = []

        def transport(method, url, payload=None):
            seen.append((method, url))
            return issues.get(int(url.rsplit("/", 1)[-1]))

        return transport, seen

    api, repo = "https://git.example/api/v1", "apex/yi"
    good = {
        "state": "open",
        "labels": [{"name": "size:M"}, {"name": "area:runtime"}],
        "milestone": {"title": "Tracking"},
    }

    # --- the footer
    footer = "Closes #4\n\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n"
    assert len(footer_problems(footer)) == 1 and "delete the line" in footer_problems(footer)[0]
    assert footer_problems("Co-Authored-By: Claude <noreply@anthropic.com>") != []
    assert footer_problems("Generated with cargo-dist; Closes #4") == [], "no assistant named"
    assert footer_problems("Claude Code is the tool this PR wires up") == [], "prose is not a credit"
    assert footer_problems("") == []

    # --- the table reader
    doc = (
        "# t\n\n## Changelog\n\n| ver | date | change |\n|---|---|---|\n"
        "| 0.2.0 | d | two |\n| 0.1.0 | d | one |\n\n"
        "## Feature ledger\n\n| feature | § | status |\n|---|---|---|\n| loop | 8.2 | core |\n"
    )
    assert [row_key(r) for r in table_rows(doc, CHANGELOG)] == ["0.2.0", "0.1.0"], doc
    assert [row_key(r) for r in table_rows(doc, LEDGER)] == ["loop"]
    # The changelog is now its own file, so its heading is an h1, not an h2.
    own = "# Changelog\n\nprose\n\n| ver | date | change |\n|---|---|---|\n| 0.3.0 | d | three |\n"
    assert [row_key(r) for r in table_rows(own, CHANGELOG)] == ["0.3.0"], own
    grown = doc.replace("| loop | 8.2 | core |", "| loop | 8.2 | core |\n| plan | 6 | core |")
    assert [row_key(r) for r in added_rows(doc, grown, LEDGER)] == ["plan"]
    # A row reworded in place is not a new row, and a diff would say it was.
    reworded = doc.replace("| loop | 8.2 | core |", "| loop | 8.2 | gated |")
    assert added_rows(doc, reworded, LEDGER) == []

    # --- the citation reader
    assert cited(repo, "Closes #4 and refs #5, Refs #4") == [4, 5]
    assert cited(repo, "closes apex/yi#7") == [7]
    assert cited(repo, "Closes other/repo#7") == [], "another register is not this one"
    assert cited(repo, "see issue 7 (#7)") == [], "a bare number is not a citation"
    assert cited(repo, None) == []

    # --- exempt: nothing added, nothing grown. Silent, and the forge is not asked.
    transport, seen = forge({})
    assert body_problems(transport, api, repo, "", [], [], 12) == []
    assert body_problems(transport, api, repo, "", [], ["| 0.2.0 | d | x |"], -400) == []
    assert seen == [], seen

    # --- required and uncited, by either trigger
    transport, seen = forge({})
    errs = body_problems(transport, api, repo, "no citation here", ["plan panel"], [], 0)
    assert "feature-ledger row `plan panel`" in errs[0], errs
    assert any("fgj issue create" in e for e in errs), errs
    assert seen == [], "an uncited body has nothing to ask the forge about"
    errs = body_problems(transport, api, repo, "", [], [], FREE_BAND + 1)
    assert f"+{FREE_BAND + 1}, past the free band" in errs[0], errs
    # A citation of another repository's register does not satisfy this one.
    errs = body_problems(transport, api, repo, "Closes other/repo#3", [], [], 900)
    assert errs and "names no issue" in errs[0], errs

    # --- cited and clean
    transport, seen = forge({4: good})
    assert body_problems(transport, api, repo, "Closes #4", ["row"], [], 0) == []
    assert seen == [("GET", f"{api}/repos/{repo}/issues/4")], seen
    assert body_problems(transport, api, repo, "Refs #4", ["row"], [], 0) == []

    # --- each issue defect, each with the fgj command that fixes it
    def only(issue, body="Closes #4"):
        transport, _ = forge({4: issue})
        return body_problems(transport, api, repo, body, ["row"], [], 0)

    errs = only(None)
    assert "not an issue" in errs[0] and "fgj issue list" in errs[1], errs
    errs = only(dict(good, state="closed"))
    assert "is closed" in errs[0] and "fgj issue reopen 4" in errs[1], errs
    errs = only(dict(good, labels=[{"name": "area:runtime"}]))
    assert "no `size:` label" in errs[0] and "--add-label size:M" in errs[1], errs
    errs = only(dict(good, labels=[{"name": "size:S"}, {"name": "size:L"}, {"name": "area:tui"}]))
    assert "2 size labels" in errs[0] and "--remove-label size:S" in errs[1], errs
    errs = only(dict(good, labels=[{"name": "size:M"}]))
    assert "no `area:` label" in errs[0], errs
    errs = only(dict(good, milestone=None))
    assert "no milestone" in errs[0] and "milestone=<id>" in errs[1], errs
    # Every cited number is judged, not just the first.
    transport, seen = forge({4: good, 5: dict(good, milestone=None)})
    errs = body_problems(transport, api, repo, "Refs #4, closes #5", ["row"], [], 0)
    assert len(errs) == 2 and errs[0].startswith("#5 has no milestone"), errs

    # --- the changelog row has to carry the same number
    transport, _ = forge({4: good})
    row = "| 0.117.0 | 2026-09-02 | a thing landed |"
    errs = body_problems(transport, api, repo, "Closes #4", ["row"], [row], 0)
    assert errs[0].startswith("the 0.117.0 changelog row cites no issue"), errs
    transport, _ = forge({4: good})
    cite = "| 0.117.0 | 2026-09-02 | a thing landed (#4) |"
    assert body_problems(transport, api, repo, "Closes #4", ["row"], [cite], 0) == []

    # --- the title rule is check_commit_style's, imported, not restated
    assert subject_errors("Gate a pull request's issue citation") == []
    assert subject_errors("Added the gate."), "a bad title must still be caught here"
    print("ok   pr_metadata selfcheck")


def measure():
    """The three facts the rule needs, measured the way the other gates measure them."""
    ref, base = base_commit()
    if base is None:
        return None, ("no origin/main or main to diff against; this gate cannot pass blind")
    def added(path, heading):
        return added_rows(git("show", f"{base}:{path}").stdout, (ROOT / path).read_text(), heading)

    rows = diff_stats(base)
    return (added(ARCH, LEDGER), added(LOG, CHANGELOG), tally(rows)[2]), None


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    errs = [f"title: {e}" for e in subject_errors(os.environ.get("PR_TITLE", "").strip())]
    measured, why = measure()
    if why:
        fail([why], "pr_metadata")
    ledger_added, changelog_added, src_net = measured
    errs += footer_problems(os.environ.get("PR_BODY", ""))
    errs += body_problems(
        send,
        os.environ["FORGEJO_API_URL"],
        os.environ["FORGEJO_REPOSITORY"],
        os.environ.get("PR_BODY", ""),
        [row_key(row) for row in ledger_added],
        changelog_added,
        src_net,
    )
    fail(errs, "pr_metadata")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
