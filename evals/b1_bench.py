#!/usr/bin/env python3
"""Stage 9 bench: does `models.selector` keep the lines a fix needs?

Paid, a few cents: each case runs `yi debug select` on a recorded command output and scores the
ranges against the gold lines a reader needs (the failure, its assertion, its location, the
summary). The gate before the selector becomes a default is recall >= 0.85 with p95 under 3 s.

    python3 evals/b1_bench.py --model openrouter/deepseek/deepseek-v4.1-flash [--reps 3]
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates/tools/tests/fixtures/reduce"

# (fixture, the command that produced it, gold line ranges, 1-based inclusive)
CASES = [
    ("pytest-verbose-one-failure.txt", "python -m pytest", [(223, 223), (413, 414), (416, 416), (418, 419)]),
    ("cargo-test-one-failure.txt", "cargo test", [(158, 158), (310, 313), (320, 320)]),
    ("compileall-one-syntax-error.txt", "python -m compileall -f pkg", [(814, 817)]),
    ("red-workspace-build.txt", "cargo build", [(726, 730), (1458, 1458)]),
]


def cost_of(answer: dict) -> float:
    """`yi debug select` prints one cost a part; an older binary printed one `cost`."""
    return sum(answer.get("costs") or []) or answer.get("cost") or 0


def lines_of(ranges) -> set[int]:
    return {line for start, end in ranges for line in range(start, end + 1)}


def recall(gold, picked) -> float:
    wanted = lines_of(gold)
    return len(wanted & lines_of(picked)) / len(wanted)


def p95(values: list[float]) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(0.95 * (len(ordered) - 1))))]


def run(yi: str, model: str, reps: int) -> dict:
    rows = []
    for fixture, command, gold in CASES:
        for rep in range(reps):
            out = subprocess.run(
                [yi, "debug", "select", str(FIXTURES / fixture), command, "--model", model],
                capture_output=True, text=True, check=False,
            )
            try:
                answer = json.loads(out.stdout.strip().splitlines()[-1])
            except (IndexError, json.JSONDecodeError):
                answer = {"error": f"exit {out.returncode}: {out.stderr.strip()[-200:]}"}
            if out.returncode != 0:
                answer.setdefault("error", f"exit {out.returncode}")
            picked = [tuple(pair) for pair in answer.get("ranges", [])]
            rows.append({
                "case": fixture, "rep": rep, "recall": recall(gold, picked), "ms": answer.get("ms", 0),
                "cost": cost_of(answer), "lines": len(lines_of(picked)), "error": answer.get("error"),
            })
    score = {
        "model": model, "runs": len(rows),
        "recall": sum(row["recall"] for row in rows) / len(rows),
        "p95_ms": p95([row["ms"] for row in rows if row["ms"]] or [0]),
        "cost": sum(row["cost"] for row in rows),
        "errors": sum(1 for row in rows if row["error"]),
    }
    score["gate"] = score["recall"] >= 0.85 and score["p95_ms"] < 3000
    return {"score": score, "rows": rows}


def selftest() -> None:
    assert recall([(10, 12)], [(11, 20)]) == 2 / 3
    assert recall([(1, 1), (5, 5)], []) == 0.0
    assert p95([100, 200, 300, 4000]) == 4000
    assert lines_of([(3, 4), (4, 5)]) == {3, 4, 5}
    assert cost_of({"costs": [0.25, 0.5, 0]}) == 0.75 and cost_of({"cost": 0.1}) == 0.1
    for fixture, _, gold in CASES:
        total = len((FIXTURES / fixture).read_text().splitlines())
        assert max(end for _, end in gold) <= total, f"{fixture}: a gold line past its {total} lines"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--model")
    parser.add_argument("--reps", type=int, default=3)
    parser.add_argument("--yi", default=str(ROOT / "target/debug/yi"))
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        selftest()
        print("ok   b1_bench selftest")
        return
    if not args.model:
        sys.exit("b1_bench: --model is required")
    print(json.dumps(run(args.yi, args.model, args.reps), indent=1))


if __name__ == "__main__":
    main()
