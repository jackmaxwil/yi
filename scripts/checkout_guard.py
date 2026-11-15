#!/usr/bin/env python3
"""Refuse to discard or overwrite uncommitted work in a checkout the caller did not create.

Incident: on 2026-11-06 a session discarded the owner's uncommitted README edits in the main checkout, and
land-fix deletes whatever sits at its worktree path, a worktree an earlier run kept for a person included.
A checkout is the caller's while it carries the mark `made` wrote in its git dir and its HEAD and
uncommitted paths still equal the snapshot the mark holds; work added after the mark is a person's.
`kept` hands it to a person with a mark of its own, after which no script may discard it, clean or not.

    checkout_guard.py made <dir> --by NAME    mark a checkout NAME just created
    checkout_guard.py kept <dir>              leave it for a person: no script discards it again
    checkout_guard.py check <dir> --by NAME   exit 1, naming the work, when NAME may not discard <dir>

It guards the scripts that call it; a person or agent running git by hand is outside it.
"""
import argparse
import contextlib
import io
import pathlib
import subprocess
import sys
import tempfile

MARK, HELD = "yi-made-by", "yi-kept-for-a-person"


def git(d, *args):
    return subprocess.run(["git", "-C", str(d), *args], capture_output=True, text=True)


def git_dir(d):
    """The checkout's own git dir, or None when `d` is not the top of a checkout (a stray dir inside one is not)."""
    top = git(d, "rev-parse", "--show-toplevel")
    if top.returncode or pathlib.Path(top.stdout.strip()).resolve() != pathlib.Path(d).resolve():
        return None
    gd = git(d, "rev-parse", "--absolute-git-dir")
    return None if gd.returncode or not gd.stdout.strip() else pathlib.Path(gd.stdout.strip())


def marked(d):
    if (gd := git_dir(d)) is None:
        raise SystemExit(f"checkout_guard: {d} is not the top of a git checkout, so it carries no mark")
    return gd


def state(d):
    """HEAD and the NUL-separated status of `d`, or None when git cannot read either; an unborn HEAD is empty."""
    head = git(d, "rev-parse", "--verify", "-q", "HEAD")
    status = git(d, "status", "--porcelain", "-z", "--untracked-files=all")
    return None if head.returncode > 1 or status.returncode else (head.stdout.strip(), status.stdout)


def made(d, by):
    gd = marked(d)
    head, status = state(d) or ("", "")
    (gd / MARK).write_text(f"{by}\n{head}\n{status}")


def marked_by(mark):
    """The script a mark names and the (HEAD, status) it snapshotted, None for a mark written without one."""
    if not mark.is_file():
        return None, None
    by, _, rest = mark.read_text().partition("\n")
    head, sep, status = rest.partition("\n")
    return by.strip(), (head, status) if sep else None


def drift(was, now):
    new = sorted({e[3:] for e in now[1].split("\0") if e} - {e[3:] for e in was[1].split("\0") if e})
    moved = [f"HEAD moved from {was[0][:9] or 'unborn'} to {now[0][:9] or 'unborn'}"] * (was[0] != now[0])
    return "; ".join(moved + ([f"new paths {', '.join(new[:5])}" + (f" and {len(new) - 5} more" if len(new) > 5 else "")] if new else [])) or "its uncommitted state changed"


def kept(d):
    gd = marked(d)
    (gd / MARK).unlink(missing_ok=True)
    (gd / HELD).write_text("a person\n")


def refusal(d, by, off=""):
    """Why `by` may not discard `d`, or None. A missing or empty directory holds nothing to lose."""
    d = pathlib.Path(d)
    if not d.is_dir() or not any(d.iterdir()):
        return None
    gd = git_dir(d)
    if gd is None:
        # Files that are in no checkout of their own cannot be told from scratch, so they are a person's.
        return None if off == "bare" else f"{d} is no git checkout, so its files cannot be told from scratch; a person removes it"
    if off != "held" and (gd / HELD).is_file():
        return f"{d} was kept for a person, so no script discards it; a person removes it"
    owner, snap = marked_by(gd / MARK)
    if off != "owner" and owner == by:
        now = state(d)
        if off == "snapshot" or (snap is not None and now == snap):
            return None
        if snap is not None and now is not None:
            return f"{d} changed after {by} made it ({drift(snap, now)}); commit or move that work first"
    untracked = "no" if off == "untracked" else "all"
    status = git(d, "status", "--porcelain", f"--untracked-files={untracked}")
    if status.returncode and off != "status":
        # Incident: a killed run's index.lock or torn index makes status exit 128 with empty stdout, which read as clean.
        return f"git could not read the uncommitted work in {d} ({status.stderr.strip()[:120]}); a person looks first"
    work = status.stdout.splitlines()
    if not work or off == "dirty":
        return None
    shown = ", ".join(line[3:] for line in work[:5]) + (f" and {len(work) - 5} more" if len(work) > 5 else "")
    return f"{d} holds {len(work)} uncommitted path(s) ({shown}) in a checkout {by} did not make; commit or move them first"


def selfcheck():
    errs = []
    for off in ("", "owner", "snapshot", "dirty", "untracked", "bare", "held", "status"):
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
            (repo / "later.txt").write_text("a person's edit after the mark\n")
            if not refusal(repo, "land-fix", off):
                bad.append("work added to a marked checkout after the mark may be discarded")
            (repo / "later.txt").unlink()
            kept(repo)
            if not refusal(repo, "land-fix", off):
                bad.append("a checkout kept for a person may be discarded")
            (repo / "README.md").unlink()
            if not refusal(repo, "land-fix", off):
                bad.append("a kept checkout may be discarded once its work is committed")
            (repo / ".git" / HELD).unlink()
            (repo / "new.txt").write_text("an untracked file\n")
            if not refusal(repo, "land-fix", off):
                bad.append("an untracked file is not counted as work")
            (repo / "new.txt").unlink()
            if refusal(repo, "land-fix", off):
                bad.append("a clean checkout is refused")
            (repo / ".git" / "index").write_text("torn by a killed run\n")
            if not refusal(repo, "land-fix", off):
                bad.append("a checkout whose status git cannot read may be discarded")
            (repo / ".git" / "index").unlink()
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
    return errs + entry_errors()


def entry_errors():
    errs = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp)
        git(repo, "init", "-q")
        git(repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a")
        (repo / "README.md").write_text("the owner's edit\n")
        args = ["--by", "land-fix"]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = main(["check", str(repo), *args])
        if code != 1 or "README.md" not in out.getvalue():
            errs.append(f"check on a dirty checkout exited {code} and said {out.getvalue()!r}")
        main(["made", str(repo), *args])
        (repo / "later.txt").write_text("after\n")
        with contextlib.redirect_stdout(io.StringIO()):
            if main(["check", str(repo), *args]) != 1:
                errs.append("check let a script discard work added after made")
        (repo / "later.txt").unlink()
        if main(["check", str(repo), *args]) != 0:
            errs.append("check refused a checkout still equal to what made marked")
        main(["kept", str(repo)])
        with contextlib.redirect_stdout(io.StringIO()):
            if main(["check", str(repo), *args]) != 1:
                errs.append("check let a script discard a checkout kept for a person")
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
