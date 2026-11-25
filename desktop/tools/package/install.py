#!/usr/bin/env python3
"""Put the packaged app where this Mac runs it from, keeping the one it replaces.

Usage: install.py [--to DIR] [--selfcheck]. Copies target/package/Yi Desktop.app into DIR
(default ~/Applications), after moving the bundle already there to target/package/previous/, so
one step back is a copy away. Refuses while the installed app is running: a bundle swapped under
a live process leaves it reading files that no longer match its code. Exit 0 with the installed
path and version, 1 with why not.
"""

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from bundle import BUNDLE, EXE, NAME, ROOT, read_info

PREVIOUS = ROOT / "target/package/previous"


def running(app):
    """Whether a process command line names the executable inside `app`; raises when ps fails."""
    exe = str(app / "Contents/MacOS" / EXE)
    listed = subprocess.run(["ps", "-axww", "-o", "command="], capture_output=True, text=True, check=False)
    if listed.returncode:
        raise RuntimeError(f"ps exited {listed.returncode}: {listed.stderr.strip()}")
    return any(exe in line for line in listed.stdout.splitlines())


def install(bundle, to, previous, is_running=running):
    """None once `bundle` stands in `to`, the bundle it replaced in `previous`; else why not."""
    if not (bundle / "Contents/MacOS" / EXE).is_file():
        return f"no packaged app at {bundle}; run `just package` first"
    app = to / bundle.name
    if app.exists():
        try:
            busy = is_running(app)
        except (OSError, RuntimeError) as err:
            return f"cannot tell whether {app} is running, so nothing was installed: {err}"
        if busy:
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
    return read_info(app).get("CFBundleShortVersionString", "no version")


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

        regex_home = root / "Apps [(+"
        live = regex_home / bundle.name
        (live / "Contents/MacOS").mkdir(parents=True)
        live_exe = live / "Contents/MacOS" / EXE
        live_exe.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(60)\n")
        live_exe.chmod(0o755)
        if running(live):
            bad.append("an app that is not running was seen running")
        proc = subprocess.Popen([str(live_exe)])
        try:
            if not running(live):
                bad.append("a running app at a path with regex characters was not seen running")
            if install(bundle, regex_home, previous, running) is None:
                bad.append("a running app at a path with regex characters was replaced")
        finally:
            proc.kill()
            proc.wait()
        if running(live):
            bad.append("a killed app was still seen running")
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
