#!/usr/bin/env python3
"""How the real window feels: the long conversation driven on the wall clock, every frame logged.

Usage: feel.py [--material synced|unsynced|off]. Run by `just feel-live`. It builds the
`feel_live` example with Rune's frame log, opens a real window for about twelve seconds (it
takes the keyboard: a covered window draws nothing), and prints a row a phase: frames a second,
the share of frames that came more than half a refresh late, and the 95th percentile of the gap
between frames, of the wait for a drawable, and of the frame's own work. `--material` is Rune's
`RUNE_SYSTEM_MATERIAL`, to tell the glass's cost from the app's. Exit 1 when a phase drew too
few frames to judge (the window was hidden or the display slept).
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK = ROOT / "target/feel"
BIN = ROOT / "target/release/examples/feel_live"
RUN_TIMEOUT = 60
FEW = 20


def percentile(values, q):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, len(ordered) * q // 100)] / 1e6 if ordered else 0.0


def phases(frames, marks):
    """One row a phase between two marks: (name, frames, fps, late share, gap, wait, work)."""
    start = frames[0]["t"] + frames[0]["total"]
    rows = []
    for (name, begins), (_, ends) in zip(marks, marks[1:]):
        drawn = [f for f in frames if begins <= (f["t"] - start) / 1e9 < ends]
        gaps = [b["t"] - a["t"] for a, b in zip(drawn, drawn[1:])]
        # A frame is late when it follows the one before by more than a refresh and a half.
        refresh = percentile(gaps, 50) * 1e6 if gaps else 0.0
        late = sum(1 for gap in gaps if gap > refresh * 1.5)
        rows.append((name, len(drawn), len(drawn) / (ends - begins),
                     100.0 * late / len(gaps) if gaps else 0.0, percentile(gaps, 95),
                     percentile([f["acquire"] for f in drawn], 95),
                     percentile([f["cpu"] for f in drawn], 95)))
    return rows


def selfcheck():
    frames = [{"t": n * 8_333_333, "total": 0, "acquire": 1_000_000, "cpu": 2_000_000} for n in range(240)]
    frames[100]["t"] += 9_000_000
    rows = phases(frames, [("a", 0.0), ("b", 1.0), ("end", 2.0)])
    first, second = rows
    assert first[0] == "a" and 119 <= first[1] <= 121, first
    assert first[3] > 0.0 and second[3] == 0.0, "the one late frame is in the first phase"
    assert abs(first[5] - 1.0) < 1e-9 and abs(first[6] - 2.0) < 1e-9, first
    assert phases(frames, [("gone", 5.0), ("end", 6.0)])[0][1] == 0
    print("feel: selfcheck ok")


def main():
    if "--selfcheck" in sys.argv:
        selfcheck()
        return 0
    material = sys.argv[sys.argv.index("--material") + 1] if "--material" in sys.argv else None
    built = subprocess.run(
        ["cargo", "build", "--release", "-q", "-p", "yi-desktop", "--example", "feel_live",
         "--features", "tooling,rune/frame-profile"], cwd=ROOT)
    if built.returncode:
        return 1
    (WORK / "home").mkdir(parents=True, exist_ok=True)
    log = WORK / "frames.jsonl"
    log.unlink(missing_ok=True)
    env = {"HOME": str(WORK / "home"), "PATH": os.environ.get("PATH", ""), "RUNE_FRAME_LOG": str(log)}
    if material:
        env["RUNE_SYSTEM_MATERIAL"] = material
    ran = subprocess.run([str(BIN)], env=env, cwd=WORK, capture_output=True, text=True, timeout=RUN_TIMEOUT)
    said = re.search(r"^feel-marks (.*)$", ran.stdout, re.M)
    if ran.returncode or said is None or not log.exists():
        print(f"feel: the window did not finish its run (exit {ran.returncode}):\n{ran.stderr[-800:]}", file=sys.stderr)
        return 1
    words = said.group(1).split()
    marks = [(words[n], float(words[n + 1])) for n in range(0, len(words), 2)]
    frames = [json.loads(line) for line in log.read_text().splitlines()]
    print(f"{'phase':8} {'frames':>6} {'fps':>6} {'late':>6} {'gap p95':>8} {'wait p95':>9} {'work p95':>9}")
    hidden = []
    for name, drawn, fps, late, gap, wait, work in phases(frames, marks):
        print(f"{name:8} {drawn:6} {fps:6.1f} {late:5.1f}% {gap:6.2f}ms {wait:7.2f}ms {work:7.2f}ms")
        if drawn < FEW:
            hidden.append(name)
    if hidden:
        print(f"feel: too few frames in {', '.join(hidden)}: the window was covered or the display slept; "
              "bring this Mac's desktop up and run again", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
