#!/usr/bin/env python3
"""Scene runner: render every registered scene headless, in parallel, and check parity scenes.

Usage: scenes.py [--only PREFIX] [--jobs N]. Writes target/scenes/<pane>/<state>.png and
.tree.json. Exit 0 when every scene rendered (and every parity check passed), 1 otherwise.
"""

import argparse
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "target/debug/yi-desktop"
OUT = ROOT / "target/scenes"
PARITY = ROOT / "tools/parity/parity.py"
TIMEOUT = 120
# A scene of a whole window is compared with the whole mockup frame it draws, bars and panes together.
WHOLE = {"shell/everything": "1"}


def run(cmd):
    """(exit code, last output lines) of a command run from the lane root."""
    try:
        done = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return 124, f"timed out after {TIMEOUT} s"
    lines = (done.stdout + done.stderr).strip().splitlines()
    # A layout Rune itself reports as broken (a scroll area with no height) fails the render.
    warned = [line for line in lines if line.startswith("rune: scroll viewport") or line.startswith("rune: growth stack")]
    if warned and done.returncode == 0:
        return 3, warned[0][:300]
    return done.returncode, "\n".join(lines[-5:])


def render(name):
    """One problem line, or None when the scene rendered (and matched the mockup, if it is one)."""
    png, tree = OUT / f"{name}.png", OUT / f"{name}.tree.json"
    png.parent.mkdir(parents=True, exist_ok=True)
    code, tail = run([str(BIN), "--scene", name, "--snap", str(png), "--tree", str(tree)])
    if code != 0:
        return f"{name}: exit {code}: {tail}"
    if not (png.is_file() and png.stat().st_size and tree.is_file() and tree.stat().st_size):
        return f"{name}: no picture or tree written"
    slug, _, state = name.partition("/")
    if name in WHOLE:
        against = ["--frame", WHOLE[name]]
    elif state == "mockup":
        against = ["--pane", slug]
    else:
        return None
    if not PARITY.exists():
        print(f"scenes: {name}: parity not installed")
        return None
    code, tail = run([sys.executable, str(PARITY), "--tree", str(tree), *against])
    return None if code == 0 else f"{name}: parity exit {code}: {tail}"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--only", default="", help="render only scenes whose name starts with this")
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    args = parser.parse_args()
    code, tail = run(["cargo", "build", "-q", "-p", "yi-desktop"])
    if code != 0:
        print(f"scenes: build failed: {tail}")
        return 1
    listed = subprocess.run([str(BIN), "--list-scenes"], cwd=ROOT, capture_output=True, text=True)
    names = [n for n in listed.stdout.split() if n.startswith(args.only)]
    if listed.returncode != 0 or not names:
        print(f"scenes: none to render (list exit {listed.returncode}, only {args.only!r})")
        return 1
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        problems = [p for p in pool.map(render, names) if p]
    for p in problems:
        print(f"scenes: {p}")
    print(f"scenes: {len(names)} rendered to {OUT.relative_to(ROOT)}, {len(problems)} failed")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
