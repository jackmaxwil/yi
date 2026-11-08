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
import tempfile

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
    out = subprocess.run(("git", "-C", str(forge_pr.ROOT)) + args, capture_output=True, text=True, errors="replace")
    if out.returncode:
        # Incident: a failed fetch left each PR's diff empty, so every open PR read as owning no file.
        raise SystemExit(f"owners: git {' '.join(args)[:100]} failed: {out.stderr.strip()[:200]}")
    return out.stdout


def changed(base, head):
    return git("diff", "--name-only", f"{base}...{head}").split()


def taken(base="origin/main", pr=None, paths=None, off=""):
    """The overlap lines for this checkout's branch (or `paths`) against every other open pull request."""
    pulls = forge_pr.open_pulls()
    if not isinstance(pulls, list):
        why = pulls.get("message") if isinstance(pulls, dict) else "a listing page answered 404"
        raise SystemExit(f"owners: the forge did not list the open pull requests: {why}")
    branch = git("rev-parse", "--abbrev-ref", "HEAD").strip()
    mine_pr = next((p["number"] for p in pulls if p["number"] == pr or p["head"]["ref"] == branch), pr)
    others = [p for p in pulls if p["number"] != mine_pr]
    if others:
        git("fetch", "-q", "origin", *(f"+refs/pull/{p['number']}/head:refs/remotes/origin/pull/{p['number']}" for p in others))
    fork = git("merge-base", base, "HEAD").strip()
    mine = paths if paths is not None else changed(base, "HEAD")
    own = set() if paths is not None else forge_pr.ahead("HEAD", base, check=True)
    shares = {int(v.split()[0][1:]) for _, fields in pending(fork) for v in fields.get("shares", [])}
    rows = []
    for p in others:
        head = f"refs/remotes/origin/pull/{p['number']}"
        rows.append((p["number"], changed(base, head), bool(own & forge_pr.ahead(head, base, check=True))))
    return overlaps(f"#{mine_pr}" if mine_pr else "this branch", mine, rows, shares, off)


def taken_case(off):
    """taken() over a real repository: #7 overlaps, #8 is stacked on this branch, #9 is shared, #10 writes a change file."""
    global pending
    saved = forge_pr.ROOT, forge_pr.open_pulls, pending
    bad = []
    with tempfile.TemporaryDirectory() as tmp:
        run = lambda *a: subprocess.run(("git", "-C", tmp, "-c", "user.name=t", "-c", "user.email=t@t") + a,
                                        capture_output=True, text=True, check=True).stdout.strip()

        def commit(*files):
            for name in files:
                (pathlib.Path(tmp) / name).parent.mkdir(parents=True, exist_ok=True)
                (pathlib.Path(tmp) / name).write_text(f"{name} {len(files)}\n")
            run("add", ".")
            run("commit", "-qm", "c")
            return run("rev-parse", "HEAD")
        run("init", "-q", "-b", "main")
        base, heads = commit("seed"), {}
        run("remote", "add", "origin", tmp)
        run("update-ref", "refs/remotes/origin/main", base)
        for n, name in ((7, "a.rs"), (9, "a.rs"), (10, "docs/changes/x.md")):
            run("checkout", "-q", "-b", f"pr{n}", base)
            heads[n] = commit(name)
        run("checkout", "-q", "-b", "mine", base)
        heads[5] = commit("a.rs", "docs/changes/x.md")
        run("checkout", "-q", "-b", "pr8")
        heads[8] = commit("c.rs")
        run("checkout", "-q", "mine")
        # The stack's base moved on after #8 was cut from it, so neither head is the other's ancestor.
        heads[5] = commit("b.rs")
        for n, sha in heads.items():
            run("update-ref", f"refs/pull/{n}/head", sha)
        # A head an earlier run fetched and the forge no longer serves: a failed fetch must not read it stale.
        run("update-ref", "refs/remotes/origin/pull/11", heads[7])
        pulls = [{"number": n, "head": {"ref": "mine" if n == 5 else f"pr{n}"}} for n in heads]
        try:
            forge_pr.ROOT, forge_pr.open_pulls = pathlib.Path(tmp), lambda: pulls
            pending = lambda fork: [("x.md", {"shares": ["#9 both edit a.rs"]})]
            said = taken(off=off)
            if said != ["#5 and #7 both change a.rs; " + WAY.format(n=7)]:
                bad.append(f"taken() over a real repository read {said}, not #7 alone")
            if not off:
                for why, listing in (("a listed PR whose head cannot be fetched", pulls + [{"number": 11, "head": {"ref": "gone"}}]),
                                     ("a listing page that answered 404", None)):
                    forge_pr.open_pulls = lambda listing=listing: listing
                    try:
                        bad.append(f"{why} was read without failing: {taken()}")
                    except SystemExit:
                        pass
        finally:
            forge_pr.ROOT, forge_pr.open_pulls, pending = saved
    return bad


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
    return bad + taken_case(off)


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
