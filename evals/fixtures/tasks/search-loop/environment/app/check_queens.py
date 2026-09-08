#!/usr/bin/env python3
"""Validate an eight-queens solution against board.json; exit 0 when it is legal."""
import json, sys
from pathlib import Path


def check(board, solution):
    n = board["size"]
    blocked = {tuple(cell) for cell in board["blocked"]}
    if not isinstance(solution, list) or len(solution) != n:
        return f"expected {n} columns, got {solution!r}"
    for row, col in enumerate(solution):
        if not isinstance(col, int) or not 0 <= col < n:
            return f"row {row}: column {col!r} is off the board"
        if (row, col) in blocked:
            return f"row {row}: cell ({row}, {col}) is blocked"
        for other in range(row):
            oc = solution[other]
            if oc == col or abs(oc - col) == row - other:
                return f"rows {other} and {row} attack each other"
    return None


def main(argv):
    root = Path(__file__).resolve().parent
    board = json.loads((root / "board.json").read_text())
    path = Path(argv[1]) if len(argv) > 1 else root / "solution.json"
    try:
        solution = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        print(f"no solution: {error}")
        return 1
    problem = check(board, solution)
    print("ok" if problem is None else problem)
    return 0 if problem is None else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
