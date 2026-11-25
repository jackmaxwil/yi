#!/usr/bin/env python3
"""Put the packaged app where this Mac runs it from, keeping the one it replaces.

Usage: install.py [--to DIR] [--selfcheck]. Copies target/package/Yi Desktop.app into DIR
(default ~/Applications), after moving the bundle already there to target/package/previous/, so
one step back is a copy away. Refuses while the installed app is running: a bundle swapped under
a live process leaves it reading files that no longer match its code. Exit 0 with the installed
path and version, 1 with why not.
"""

import argparse
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NAME, EXE = "Yi Desktop", "yi-desktop"
BUNDLE = ROOT / "target/package" / f"{NAME}.app"
PREVIOUS = ROOT / "target/package/previous"


def running(app):
    """Whether some process runs the executable inside `app`."""
    exe = str(app / "Contents/MacOS" / EXE)
    return subprocess.run(["pgrep", "-f", exe], capture_output=True, text=True, check=False).returncode == 0


def install(bundle, to, previous, is_running=running):
    """None once `bundle` stands in `to`, the bundle it replaced in `previous`; else why not."""
    if not (bundle / "Contents/MacOS" / EXE).is_file():
        return f"no packaged app at {bundle}; run `just package` first"
    app = to / bundle.name
    if app.exists() and is_running(app):
        return f"{app} is running; quit it, then install"
    to.mkdir(parents=True, exist_ok=True)
    staged = to / f".{bundle.name}.new"
    if staged.exists():
        shutil.rmtree(staged)
    shutil.copytree(bundle, staged, symlinks=True)
    if app.exists():
        kept = previous / bundle.name
        if kept.exists():
            shutil.rmtree(kept)
        previous.mkdir(parents=True, exist_ok=True)
        shutil.move(str(app), str(kept))
    staged.rename(app)
    return None


def version(app):
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    return info.get("CFBundleShortVersionString", "no version")


def selfcheck():
    """A first install, a second over it, a running app and a missing bundle, each in a scratch folder."""
    bad = []
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        bundle, to, previous = root / "built" / f"{NAME}.app", root / "Applications", root / "previous"

        def build(words):
            (bundle / "Contents/MacOS").mkdir(parents=True, exist_ok=True)
            (bundle / "Contents/MacOS" / EXE).write_text(words)

        installed = lambda: (to / bundle.name / "Contents/MacOS" / EXE).read_text()
        if install(bundle, to, previous, lambda app: False) is None:
            bad.append("a bundle that was never packaged was installed")
        build("one")
        if install(bundle, to, previous, lambda app: False) or installed() != "one":
            bad.append("a first install did not land")
        build("two")
        if install(bundle, to, previous, lambda app: True) is None or installed() != "one":
            bad.append("a running app was replaced")
        if install(bundle, to, previous, lambda app: False) or installed() != "two":
            bad.append("a second install did not replace the first")
        if (previous / bundle.name / "Contents/MacOS" / EXE).read_text() != "one":
            bad.append("the replaced bundle was not kept")
        if list(to.glob(".*.new")):
            bad.append("a staged copy was left behind")
    print("\n".join(f"FAIL install selfcheck: {b}" for b in bad) or "ok   install selfcheck")
    return 1 if bad else 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--to", type=Path, default=Path.home() / "Applications")
    parser.add_argument("--selfcheck", action="store_true")
    args = parser.parse_args()
    if args.selfcheck:
        return selfcheck()
    if sys.platform != "darwin":
        print("install: Yi Desktop installs on a Mac")
        return 1
    if why := install(BUNDLE, args.to, PREVIOUS):
        print(f"install: {why}")
        return 1
    app = args.to / BUNDLE.name
    print(f"install: {app} is version {version(app)}; the one it replaced is in {PREVIOUS}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
