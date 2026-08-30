#!/usr/bin/env python3
"""Sum `usage.cost.total` over every yi.jsonl a harbor run synced back.

The slice cap is enforced between tasks, so the driver needs the spend so far
as a bare number on stdout and nothing else. With `--soft`/`--hard` this is
also the gate: exit 2 means stop, and a spend that cannot be computed raises
rather than answering 0 -- a money gate defaults to stopping.
"""

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "adapters"))

import yi_usage  # noqa: E402


class Unmeasurable(Exception):
    """A run whose spend cannot be read. Never a number, so no caller can sum
    it into a total that looks safe."""


def spent(runs_dir):
    total = 0.0
    for path in sorted(Path(runs_dir).rglob("yi.jsonl")):
        usage = yi_usage.parse_events(path)
        # D79: a turn whose stream died before its usage chunk prices at zero,
        # so summing it lets an unmeasurable run walk under the cap forever.
        if usage.get("costUnknownTurns"):
            raise Unmeasurable(
                f"{path}: {usage['costUnknownTurns']} turn(s) reported no usage"
            )
        total += usage.get("costUsd") or 0.0
    return total


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("runs_dir", nargs="?", default="runs")
    parser.add_argument("--soft", type=float, help="refuse to start another task at")
    parser.add_argument("--hard", type=float, help="stop the stream at")
    args = parser.parse_args(argv)
    try:
        total = spent(args.runs_dir)
    except Unmeasurable as unreadable:
        print(f"STOP: spend cannot be measured -- {unreadable}", file=sys.stderr)
        return 2
    print(f"{total:.6f}")
    for name, cap in (("hard", args.hard), ("soft", args.soft)):
        if cap is not None and total >= cap:
            print(f"STOP: {name} cap ${cap:g} reached at ${total:.6f}", file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
