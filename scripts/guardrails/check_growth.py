#!/usr/bin/env python3
"""Net src growth is budgeted: <= +150 lines rides free, past it the current
version's changelog row must carry a `growth +N:` memo, past +2000 that row must
also cite the decision row the landing claimed.
Both bands come from history, which --calibrate reprints: over 95 versions the
free band lands almost exactly on the seam between the two regimes — 46 organic
bumps under it, 49 landings over — and 7 of those landings clear +2000."""
import json, re, statistics, subprocess, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, src_files, fail

FREE = 150
DROW = 2000
BASELINE = BASE / "src_loc.json"
ARCH = ROOT / "docs/ARCHITECTURE.md"
VERSION = re.compile(r"^version:\s*(\S+)", re.M)
MEMO = re.compile(r"growth \+(\d+):")
CITE = re.compile(r"\bD\d+\b")
# Spelled as a filter, not left to the pathspec: git's globs cross a slash, so
# the pathspec alone would count a nested crate the size ratchets never see.
TRACKED = re.compile(r"^crates/[^/]+/src/.+\.rs$")


def git(*args):
    return subprocess.run(
        ("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False
    )


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
        if TRACKED.match(head.partition(":")[2]):
            total += int(count)
    return total


def version_at(rev):
    found = VERSION.search(git("show", f"{rev}:docs/ARCHITECTURE.md").stdout)
    return found.group(1) if found else None


def changelog_row(version):
    for line in ARCH.read_text().splitlines():
        if line.startswith(f"| {version} |"):
            return line
    return None


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


if "--calibrate" in sys.argv:
    calibrate()
    sys.exit(0)

header = VERSION.search(ARCH.read_text())
if header is None:
    fail([f"no `version:` header in {ARCH.relative_to(ROOT)}"], "growth")
version = header.group(1)
now = measured()
base = json.loads(BASELINE.read_text()) if BASELINE.exists() else None

if "--update" in sys.argv:
    BASELINE.write_text(json.dumps({"version": version, "loc": now}, indent=2) + "\n")
    was = f"{base['loc']} (version {base['version']})" if base else "unseeded"
    print(f"src LOC {was} -> {now} (version {version})")
    sys.exit(0)
# Deleting the baseline is not a way to pass: an unseeded ratchet is a red one.
if base is None:
    fail([f"{BASELINE.name} missing; seed it: check_growth.py --update"], "growth")

delta = now - base["loc"]
report = f"{now} lines, {delta:+d} since {base['version']}"
errs = []
if delta > FREE:
    row = changelog_row(version)
    if version == base["version"]:
        errs.append(f"{delta:+d} crosses the free band of +{FREE} with no version bump")
        errs.append(f"  a memo lives in a version's own changelog row: bump past {version} and write it")
    elif row is None:
        errs.append(f"{delta:+d} is past +{FREE} and {version} has no changelog row to carry the memo")
    else:
        if not MEMO.search(row):
            errs.append(f"{delta:+d} is past +{FREE}: the {version} row needs a `growth +{delta}:` clause")
            errs.append("  naming the measured number and what was weighed for deletion")
        if delta > DROW and not CITE.search(row):
            errs.append(f"{delta:+d} is past +{DROW}: the {version} row must also cite the D-row it claimed")
fail(errs, f"growth ({report}, free band +{FREE})")
