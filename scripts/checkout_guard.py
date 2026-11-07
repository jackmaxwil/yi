#!/usr/bin/env python3
"""Refuse to discard or overwrite uncommitted work in a checkout the caller did not create.

Incident: on 2026-11-06 a session discarded the owner's uncommitted README edits in the main checkout, and
land-fix deletes whatever sits at its worktree path, a worktree an earlier run kept for a person included.
A checkout is the caller's when it carries the mark `made` wrote in its git dir; `kept` hands it to a
person, after which no script may discard it.

    checkout_guard.py made <dir> --by NAME    mark a checkout NAME just created
    checkout_guard.py kept <dir>              leave it for a person: the mark goes
    checkout_guard.py check <dir> --by NAME   exit 1, naming the work, when NAME may not discard <dir>

It guards the scripts that call it; a person or agent running git by hand is outside it.
"""
import argparse
import pathlib
import subprocess
import sys
import tempfile

MARK = "yi-made-by"


def git(d, *args):
    return subprocess.run(["git", "-C", str(d), *args], capture_output=True, text=True)


def git_dir(d):
    """The checkout's own git dir, or None when `d` is not the top of a checkout (a stray dir inside one is not)."""
    top = git(d, "rev-parse", "--show-toplevel")
    if top.returncode or pathlib.Path(top.stdout.strip()).resolve() != pathlib.Path(d).resolve():
        return None
    return pathlib.Path(git(d, "rev-parse", "--absolute-git-dir").stdout.strip())


def made(d, by):
    (git_dir(d) / MARK).write_text(by + "\n")


def kept(d):
    (git_dir(d) / MARK).unlink(missing_ok=True)


def refusal(d, by, off=""):
    """Why `by` may not discard `d`, or None. A missing or empty directory holds nothing to lose."""
    d = pathlib.Path(d)
    if not d.is_dir() or not any(d.iterdir()):
        return None
    gd = git_dir(d)
    if gd is None:
        # Files that are in no checkout of their own cannot be told from scratch, so they are a person's.
        return None if off == "bare" else f"{d} is no git checkout, so its files cannot be told from scratch; a person removes it"
    mark = gd / MARK
    if off != "owner" and mark.is_file() and mark.read_text().strip() == by:
        return None
    untracked = "no" if off == "untracked" else "all"
    work = git(d, "status", "--porcelain", f"--untracked-files={untracked}").stdout.splitlines()
    if not work or off == "dirty":
        return None
    shown = ", ".join(line[3:] for line in work[:5]) + (f" and {len(work) - 5} more" if len(work) > 5 else "")
    return f"{d} holds {len(work)} uncommitted path(s) ({shown}) in a checkout {by} did not make; commit or move them first"


def selfcheck():
    errs = []
    for off in ("", "owner", "dirty", "untracked", "bare"):
        bad = []
        with tempfile.TemporaryDirectory() as tmp:
            repo = pathlib.Path(tmp) / "repo"
            repo.mkdir()
            git(repo, "init", "-q")
            git(repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a")
            (repo / "README.md").write_text("the owner's edit\n")
            if not refusal(repo, "land-fix", off):
                bad.append("a dirty checkout the caller did not make may be discarded")
            made(repo, "land-fix")
            if refusal(repo, "land-fix", off):
                bad.append("the caller's own dirty checkout is refused")
            if not refusal(repo, "conflict-fix", off):
                bad.append("another script may discard a checkout land-fix made")
            kept(repo)
            if not refusal(repo, "land-fix", off):
                bad.append("a checkout kept for a person may be discarded")
            (repo / "README.md").unlink()
            (repo / "new.txt").write_text("an untracked file\n")
            if not refusal(repo, "land-fix", off):
                bad.append("an untracked file is not counted as work")
            (repo / "new.txt").unlink()
            if refusal(repo, "land-fix", off):
                bad.append("a clean checkout is refused")
            lane = pathlib.Path(tmp) / "lane"
            git(repo, "worktree", "add", "-q", str(lane))
            (lane / "x.txt").write_text("lane work\n")
            if not refusal(lane, "land-fix", off):
                bad.append("a dirty linked worktree may be discarded")
            stray = repo / "stray"
            stray.mkdir()
            (stray / "left.txt").write_text("a killed run's files\n")
            if not refusal(stray, "land-fix", off):
                bad.append("a directory that is no checkout of its own may be discarded")
            if refusal(pathlib.Path(tmp) / "missing", "land-fix", off):
                bad.append("a missing directory is refused")
        if off and not bad:
            errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        if not off:
            errs += bad
    return errs


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("verb", choices=("made", "kept", "check"))
    ap.add_argument("dir")
    ap.add_argument("--by")
    if argv == ["--selfcheck"]:
        errs = selfcheck()
        print("\n".join(f"FAIL checkout_guard selfcheck: {e}" for e in errs) or "ok   checkout_guard selfcheck")
        return 1 if errs else 0
    args = ap.parse_args(argv)
    if args.verb != "kept" and not args.by:
        ap.error(f"{args.verb} needs --by")
    if args.verb == "made":
        made(args.dir, args.by)
    elif args.verb == "kept":
        kept(args.dir)
    elif why := refusal(args.dir, args.by):
        print(f"checkout_guard: {why}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
