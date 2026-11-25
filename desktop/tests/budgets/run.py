#!/usr/bin/env python3
"""Judge the speed budgets: three rounds of every measurement, the worst against budgets.json.

One run of the binary comes first and is not judged (`first_exec_ms` in target/budgets/last.json).

Usage: run.py [--record]. Run by `just budgets` in the release profile. Exit 1 when a number is
past its budget. --record rewrites each `measured`, and the `budget` of a row whose basis is
"worst of three plus a quarter"; a row with a stated basis keeps its budget.
"""

import json
import math
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TABLE = ROOT / "tests/budgets/budgets.json"
LAST = ROOT / "target/budgets/last.json"
BIN = ROOT / "target/release/yi-desktop"
ROUNDS = 3


def in_process():
    """The numbers the in-process tests print: the budgets, then the long conversation's."""
    got = {}
    for test in ("budgets", "feel"):
        run = subprocess.run(
            ["cargo", "test", "--release", "-p", "yi-desktop", "--features", "tooling", "--test", test, "--",
             "--test-threads=1", "--nocapture"],
            cwd=ROOT, capture_output=True, text=True)
        if run.returncode:
            sys.exit(f"budgets: the in-process {test} tests failed:\n{run.stdout[-1500:]}\n{run.stderr[-1500:]}")
        got.update({m[1]: float(m[2]) for m in re.finditer(r"^budget (\S+) (\S+)$", run.stdout, re.M)})
    return got


def first_frame():
    """Process start to the first GPU-complete picture of the whole window, headless."""
    out = subprocess.run(
        [str(BIN), "--scene", "shell/everything", "--scale", "1", "--first-pixels",
         "--snap", str(ROOT / "target/budgets/first.png")],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout
    line = next(l for l in out.splitlines() if l.startswith("rune-first-pixels "))
    return json.loads(line.removeprefix("rune-first-pixels "))["headless_ms"]


def main():
    (ROOT / "target/budgets").mkdir(parents=True, exist_ok=True)
    # The first run of a newly linked binary pays the Mac's one-time check of it (161 ms where
    # the next six took 44 to 51): it is recorded beside the rounds and never judged.
    first_exec = first_frame()
    rounds = []
    for _ in range(ROUNDS):
        got = in_process()
        got["start_to_first_frame_ms"] = first_frame()
        rounds.append(got)
    worst = {k: max(r[k] for r in rounds) for k in rounds[0]}
    LAST.write_text(json.dumps({**worst, "first_exec_ms": first_exec}, indent=1, sort_keys=True) + "\n")
    table = json.loads(TABLE.read_text())
    failed = []
    for name, row in table["budgets"].items():
        number = worst.get(name)
        if number is None:
            failed.append(f"{name}: not measured")
            continue
        if "--record" in sys.argv:
            row["measured"] = round(number, 3)
            if row["basis"] == "worst of three plus a quarter":
                row["budget"] = max(math.ceil(number * 1.25), 1) if number else 0
        if number > row["budget"]:
            failed.append(f"{name}: {number:.3f} {row['unit']} is past the budget {row['budget']}")
        print(f"{name}: {number:.3f} {row['unit']} (budget {row['budget']})")
    if "--record" in sys.argv:
        TABLE.write_text(json.dumps(table, indent=1) + "\n")
    for line in failed:
        print(f"budgets: {line}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
