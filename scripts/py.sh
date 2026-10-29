#!/usr/bin/env bash
# The one interpreter ladder: the guardrail gate and the quick-test recipes both read it.
set -u
PY="${PY:-$(command -v python3.13 || command -v python3.12 || command -v python3.11 || command -v python3)}"
"$PY" -c "import tomllib" 2>/dev/null || { echo "FAIL: $PY lacks tomllib (need python >= 3.11)"; exit 1; }
printf '%s\n' "$PY"
