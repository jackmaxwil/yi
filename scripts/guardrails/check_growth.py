#!/usr/bin/env python3
"""Net src growth is budgeted: <= +150 lines over the fork point rides free; past it a change
file this branch adds carries `growth: +N <memo>` naming the measured number and what was
weighed for deletion, and past +2000 that file also carries a `decision:`.
Both bands come from history, which --calibrate reprints: over 95 versions the
free band lands almost exactly on the seam between the two regimes — 46 organic
bumps under it, 49 landings over — and 7 of those landings clear +2000."""
import re, statistics, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import SRC, src_files, fail, fork, git, no_fork, FREE_BAND as FREE
from check_changes import GROWTH, pending

DROW = 2000
VERSION = re.compile(r"^version:\s*(\S+)", re.M)


def measured():
    return sum(len(f.read_text().splitlines()) for f in src_files())


def loc_at(rev):
    """Empty pattern-free `^` matches every line; rc 1 only means the rev holds no such file."""
    got = git("grep", "-c", "^", rev, "--", "crates/*/src/*.rs")
    if got.returncode not in (0, 1):
        return None
    total = 0
    for line in got.stdout.splitlines():
        head, _, count = line.rpartition(":")
        if not count.isdigit():
            return None
        # A filter, not the pathspec: git's globs cross a slash and would count a nested crate.
        if SRC.match(head.partition(":")[2]):
            total += int(count)
    return total


def version_at(rev):
    found = VERSION.search(git("show", f"{rev}:docs/ARCHITECTURE.md").stdout)
    return found.group(1) if found else None




def calibrate():
    """A version opens at the oldest commit carrying it, so its growth closes at the next open."""
    opens, seen = [], None
    for rev in git("log", "--format=%H", "--", "docs/ARCHITECTURE.md").stdout.split():
        version = version_at(rev)
        if version is None:
            continue
        if version == seen:
            opens[-1][1] = rev
        else:
            opens.append([version, rev])
            seen = version
    rows, close = [], measured()
    for version, rev in opens:
        before = loc_at(f"{rev}^")
        if before is None:
            break
        rows.append((version, close - before))
        close = before
    for version, delta in rows:
        print(f"  {version:>10} {delta:+7d}")
    deltas = sorted(delta for _, delta in rows)
    over = [d for d in deltas if d > FREE]
    print(f"  {len(deltas)} versions, median {statistics.median(deltas):+.0f}, "
          f"range {deltas[0]:+d}..{deltas[-1]:+d}; {len(over)} over the +{FREE} free band, "
          f"{len([d for d in over if d > DROW])} over +{DROW}")


def unpaid(delta, changes):
    """A landing measures its memo before its last commits, so the memo's number may trail the
    final measurement by whatever rides free — and a number that never reached the free band
    was never a description of this growth."""
    if delta <= FREE:
        return []
    if not changes:
        return [f"{delta:+d} is past +{FREE} and this branch adds no change file to carry the memo",
                "  add docs/changes/<yyyy-mm-dd>-<slug>.md with `growth: +N <what was weighed for deletion>`"]
    said = [int(GROWTH.match(fields["growth"]).group(1)) for _, fields in changes
            if GROWTH.match(fields.get("growth", ""))]
    errs = []
    if not said:
        errs.append(f"{delta:+d} is past +{FREE}: a change file here needs `growth: +{delta} <memo>`")
        errs.append("  naming the measured number and what was weighed for deletion")
    elif sum(said) <= FREE or abs(sum(said) - delta) > FREE:
        errs.append(f"the change files say `growth: +{sum(said)}` but the measurement is {delta:+d}")
        errs.append(f"  a memo may trail the measured number by the free band +{FREE}, not by more")
    if delta > DROW and not any(fields["decision"] for _, fields in changes):
        errs.append(f"{delta:+d} is past +{DROW}: a change file here carries the `decision:` the landing claims")
    return errs


if "--calibrate" in sys.argv:
    calibrate()
    sys.exit(0)

base = fork()
if base is None:
    no_fork("growth")
before = loc_at(base)
if before is None:
    fail([f"could not count src lines at the fork {base[:8]}"], "growth")
delta = measured() - before
fail(unpaid(delta, pending(base)), f"growth ({delta:+d} src lines since the fork {base[:8]}, free band +{FREE})")
