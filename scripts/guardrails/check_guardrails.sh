#!/usr/bin/env bash
# Aggregator = the CI entrypoint itself (D31; jcode's aggregator was referenced by zero
# workflows). Ordered most-legible-failure-first. Prints one debt total at the end.
set -u
cd "$(dirname "$0")/../.."
PY="${PY:-$(command -v python3.13 || command -v python3.12 || command -v python3.11 || command -v python3)}"
"$PY" -c "import tomllib" 2>/dev/null || { echo "FAIL: $PY lacks tomllib (need python >= 3.11)"; exit 1; }
FAILED=0
run() { "$@" || FAILED=$((FAILED + 1)); }

run "$PY" scripts/guardrails/check_manifests.py
run "$PY" scripts/guardrails/check_boundaries.py
run "$PY" scripts/guardrails/check_filenames.py
run "$PY" scripts/guardrails/check_trailers.py
run "$PY" scripts/guardrails/check_glob_reexport.py
run "$PY" scripts/guardrails/check_panic.py
run "$PY" scripts/guardrails/check_comments.py
run "$PY" scripts/guardrails/check_file_size.py
run "$PY" scripts/guardrails/check_fn_size.py
run "$PY" scripts/guardrails/check_schemas_lock.py
run "$PY" scripts/guardrails/check_env_surface.py
run "$PY" scripts/guardrails/check_duplication.py
run "$PY" scripts/guardrails/check_test_size.py
run "$PY" scripts/guardrails/check_blob_size.py
run "$PY" scripts/guardrails/check_deps_budget.py
run "$PY" scripts/guardrails/check_request_budget.py
if [ -z "${CI:-}" ]; then
  run "$PY" scripts/guardrails/check_binary_size.py
else
  echo "skip binary_size (D70: baseline is macOS arm64; CI is another target)"
fi
# D68/D70: the two gates that read target/dist/yi are local-only — one measures
# wall clock, the other a macOS arm64 byte count. Skips are announced (design 9).
if [ -z "${CI:-}" ]; then
  run "$PY" scripts/guardrails/check_startup.py
else
  echo "skip startup (D68: local-only; CI cannot measure a 5 ms budget)"
fi

run cargo doc --workspace --no-deps --document-private-items -q

if command -v cargo-machete >/dev/null; then run cargo machete crates; else echo "FAIL machete (cargo install cargo-machete)"; FAILED=$((FAILED+1)); fi
if command -v cargo-deny >/dev/null; then run cargo deny check -s; else echo "FAIL deny (cargo install cargo-deny)"; FAILED=$((FAILED+1)); fi

echo "----"
if [ "$FAILED" -eq 0 ]; then echo "guardrails: all green"; else echo "guardrails: $FAILED failing"; exit 1; fi
