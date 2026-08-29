#!/usr/bin/env bash
# Aggregator = the gate entrypoint itself (D31; jcode's aggregator was referenced by
# nothing). Ordered most-legible-failure-first. Prints one debt total at the end.
# `--fast` drops the gates that need a build (dist binary, startup, rustdoc, the two
# cargo tools) so pre-commit stays quick; pre-push runs the whole thing.
set -u
cd "$(dirname "$0")/../.."
FAST=0
[ "${1:-}" = "--fast" ] && FAST=1
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
run "$PY" scripts/guardrails/check_crate_size.py
run "$PY" scripts/guardrails/check_fn_size.py
run "$PY" scripts/guardrails/check_schemas_lock.py
run "$PY" scripts/guardrails/check_env_surface.py
run "$PY" scripts/guardrails/check_duplication.py
run "$PY" scripts/guardrails/check_test_size.py
run "$PY" scripts/guardrails/check_blob_size.py
run "$PY" scripts/guardrails/check_deps_budget.py
run "$PY" scripts/guardrails/check_request_budget.py
run "$PY" scripts/guardrails/check_behavior.py
run "$PY" scripts/guardrails/check_prompt_examples.py
# Prose is not exempt: 1,485 comment lines are under ratchet, and the design docs
# are the reference. Config and the domain-word allowlist live in .codespellrc.
if command -v codespell >/dev/null; then run codespell; else echo "FAIL codespell (uv tool install codespell)"; FAILED=$((FAILED+1)); fi
# Both gates read target/dist/yi and both are skipped for two different reasons.
# D70: the baseline is a macOS arm64 byte count, so any other target measures a
# different binary against it. --fast skips it because it needs a dist build.
if [ "$FAST" -eq 1 ]; then
  echo "skip binary_size (--fast: needs a dist build)"
elif [ -n "${CI:-}" ]; then
  echo "skip binary_size (D70: baseline is macOS arm64; CI is another target)"
else
  run "$PY" scripts/guardrails/check_binary_size.py
fi
# D68: wall clock on a shared runner is noise against a 5 ms budget; --fast
# skips it because hyperfine is most of the run. Announced, never silent (9).
if [ "$FAST" -eq 1 ]; then
  echo "skip startup (--fast: hyperfine is most of the run)"
elif [ -n "${CI:-}" ]; then
  echo "skip startup (D68: a shared runner cannot measure a 5 ms budget)"
else
  run "$PY" scripts/guardrails/check_startup.py
fi

if [ "$FAST" -eq 0 ]; then
  run cargo doc --workspace --no-deps --document-private-items -q

  if command -v cargo-machete >/dev/null; then run cargo machete crates; else echo "FAIL machete (cargo install cargo-machete)"; FAILED=$((FAILED+1)); fi
  if command -v cargo-deny >/dev/null; then run cargo deny check -s; else echo "FAIL deny (cargo install cargo-deny)"; FAILED=$((FAILED+1)); fi
else
  echo "skip doc/machete/deny (--fast)"
fi

echo "----"
if [ "$FAILED" -eq 0 ]; then echo "guardrails: all green"; else echo "guardrails: $FAILED failing"; exit 1; fi
