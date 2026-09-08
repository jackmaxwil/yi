#!/bin/bash
# The answer is a file the checker in the workspace accepts; nothing else counts.
APP="${APP:-/app}"; LOGS="${LOGS:-/logs/verifier}"
mkdir -p "$LOGS"
if python3 "$APP/check_queens.py" "$APP/solution.json"; then echo 1 > "$LOGS/reward.txt"; exit 0; fi
echo 0 > "$LOGS/reward.txt"; exit 1
