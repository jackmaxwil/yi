#!/usr/bin/env python3
"""One owner per file: a branch may not change a file an open pull request also changes, unless one
of the two is stacked on the other or the branch's change file says `shares: #N <why>`.

Incident: #2059 and #2064 were open at once and both changed crates/console/src/app.rs and
app/chat.rs, and nothing said so until one of them conflicted. Files the recorder or a ratchet
writes for every branch (MECHANICAL) merge mechanically and own nothing.

  owners.py check [--pr N] [--paths P ...] [--base origin/main]

A new lane passes the files it will change with --paths; land-fix passes the PR it fixes with --pr.
Exit 1 names each overlap and the way past.
"""
import argparse
import json
import pathlib
import subprocess
import sys

sys.path[:0] = [str(pathlib.Path(__file__).parent), str(pathlib.Path(__file__).parent / "guardrails")]
import forge_pr  # noqa: E402
from check_changes import pending  # noqa: E402

MECHANICAL = ("docs/CHANGELOG.md", "docs/ARCHITECTURE.md", "docs/changes/", "docs/solutions/", "docs/ledger-journeys.txt",
              "scripts/guardrails/baselines/", "Cargo.lock", "AGENTS.md")
FIXTURE = pathlib.Path(__file__).parent / "guardrails/fixtures/owners/pr2059-pr2064.json"
WAY = "stack on it (branch from its head), or write `shares: #{n} <why>` in this branch's change file"


def owned(paths, off=""):
    return {p for p in paths if off == "mechanical" or not p.startswith(MECHANICAL)}


def overlaps(me, mine, others, shares, off=""):
    """`others` is (number, paths, stacked) per open pull request; one line per PR that owns a file of `mine`."""
    out = []
    for number, paths, stacked in others:
        both = sorted(owned(mine, off) & owned(paths, off))
        if both and (not stacked or off == "stack") and (number not in shares or off == "shares"):
            out.append(f"{me} and #{number} both change {', '.join(both)}; {WAY.format(n=number)}")
    return out


def git(*args):
    return subprocess.run(("git", "-C", str(forge_pr.ROOT)) + args, capture_output=True, text=True, errors="replace")


def ancestor(a, b):
    return git("merge-base", "--is-ancestor", a, b).returncode == 0


def changed(base, head):
    return git("diff", "--name-only", f"{base}...{head}").stdout.split()


def taken(base="origin/main", pr=None, paths=None):
    """The overlap lines for this checkout's branch (or `paths`) against every other open pull request."""
    branch = git("rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
    pulls = forge_pr.open_pulls()
    if isinstance(pulls, dict):
        raise SystemExit(f"owners: the forge did not list the open pull requests: {pulls.get('message')}")
    mine_pr = next((p["number"] for p in pulls if p["number"] == pr or p["head"]["ref"] == branch), pr)
    others = [p for p in pulls if p["number"] != mine_pr]
    git("fetch", "-q", "origin", *(f"+refs/pull/{p['number']}/head:refs/remotes/origin/pull/{p['number']}" for p in others))
    fork = git("merge-base", base, "HEAD").stdout.strip()
    own_commits = paths is None and fork != git("rev-parse", "HEAD").stdout.strip()
    mine = paths if paths is not None else changed(base, "HEAD")
    shares = {int(v.split()[0][1:]) for _, fields in pending(fork) for v in fields.get("shares", [])}
    rows = []
    for p in others:
        head = f"refs/remotes/origin/pull/{p['number']}"
        stacked = own_commits and (ancestor(head, "HEAD") or ancestor("HEAD", head))
        rows.append((p["number"], changed(base, head), stacked))
    return overlaps(f"#{mine_pr}" if mine_pr else "this branch", mine, rows, shares)


def selfcheck(off=""):
    real = {int(n): files for n, files in json.loads(FIXTURE.read_text()).items()}
    bad = []
    said = overlaps("#2059", real[2059], [(2064, real[2064], False)], set(), off)
    if said != ["#2059 and #2064 both change crates/console/src/app.rs, crates/console/src/app/chat.rs, "
                "crates/console/tests/drive.rs, crates/tui/src/render.rs; " + WAY.format(n=2064)]:
        bad.append(f"#2059 against #2064 read {said}, not their four shared files")
    if overlaps("#2059", real[2059], [(2064, real[2064], True)], set(), off):
        bad.append("a branch stacked on the PR it overlaps is refused")
    if overlaps("#2059", real[2059], [(2064, real[2064], False)], {2064}, off):
        bad.append("`shares: #2064` does not let the overlap through")
    if overlaps("#2059", ["docs/ledger-journeys.txt", "docs/changes/a.md"], [(2064, ["docs/ledger-journeys.txt", "docs/changes/a.md"], False)], set(), off):
        bad.append("files the recorder writes for every branch count as owned")
    return bad


def main():
    if "--selfcheck" in sys.argv:
        errs = selfcheck() + [f"selfcheck passes with the {off} check off, so it refutes nothing"
                              for off in ("stack", "shares", "mechanical") if not selfcheck(off)]
        print("\n".join(errs) or "ok   owners selfcheck")
        return 1 if errs else 0
    ap = argparse.ArgumentParser()
    ap.add_argument("verb", choices=["check"])
    ap.add_argument("--pr", type=int)
    ap.add_argument("--paths", nargs="+")
    ap.add_argument("--base", default="origin/main")
    args = ap.parse_args()
    lines = taken(args.base, args.pr, args.paths)
    print("\n".join(lines) or "owners: no open pull request changes these files")
    return 1 if lines else 0


if __name__ == "__main__":
    sys.exit(main())
