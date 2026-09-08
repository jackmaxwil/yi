#!/bin/bash
python3 - <<'PY'
import json
board = json.load(open("/app/board.json")); n = board["size"]; blocked = {tuple(c) for c in board["blocked"]}
def place(row, cols):
    if row == n: return cols
    for col in range(n):
        if (row, col) in blocked or any(c == col or abs(c - col) == row - r for r, c in enumerate(cols)): continue
        out = place(row + 1, cols + [col])
        if out: return out
json.dump(place(0, []), open("/app/solution.json", "w"))
PY
