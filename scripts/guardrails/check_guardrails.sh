#!/usr/bin/env bash
# Aggregator = the gate entrypoint itself (D31; a surveyed aggregator was referenced by
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
run "$PY" scripts/guardrails/check_commit_style.py
run "$PY" scripts/guardrails/check_glob_reexport.py
run "$PY" scripts/guardrails/check_orphans.py
run "$PY" scripts/guardrails/check_panic.py
run "$PY" scripts/guardrails/check_comments.py
run "$PY" scripts/guardrails/check_file_size.py
run "$PY" scripts/guardrails/check_crate_size.py
run "$PY" scripts/guardrails/check_fn_size.py
run "$PY" scripts/guardrails/check_schemas_lock.py
run "$PY" scripts/guardrails/check_env_surface.py
run "$PY" scripts/guardrails/check_duplication.py
run "$PY" scripts/guardrails/check_test_size.py
run "$PY" scripts/guardrails/check_test_tiers.py
run "$PY" scripts/guardrails/check_blob_size.py
run "$PY" scripts/guardrails/check_public_surface.py
run "$PY" scripts/guardrails/check_deps_budget.py
run "$PY" scripts/guardrails/check_request_budget.py
run "$PY" scripts/guardrails/check_behavior.py
run "$PY" scripts/guardrails/check_prompt_examples.py
run env PYTHONPATH=python/yi_runtime/src "$PY" -m unittest discover -q -s python/yi_runtime/tests
run "$PY" evals/selftest.py
# The mined artifacts carry session text, so §10's planted-fake redaction proof is a
# gate, not a habit: its only executable check is this flag.
run "$PY" skills/yi/session-mining/extract.py --selfcheck
# The PR narrative's net-src number is what the growth budget is argued against, and
# a path in the wrong bucket misprices it silently; only this flag exercises the split.
run "$PY" scripts/pr_body.py --selfcheck
# The tool-surface lock is equality over hashes, so a comparison that drifted would
# pass every change or refuse every run; only this flag walks the delta cases.
run "$PY" scripts/guardrails/check_request_budget.py --selfcheck
# The size-report comment is upserted by marker, and a marker that stops matching
# posts a duplicate rather than failing; only this flag exercises the routing.
run "$PY" scripts/forgejo_pr_comment.py --selfcheck
run "$PY" scripts/live_report.py --selfcheck
run "$PY" scripts/live_ledger.py --selfcheck
run "$PY" scripts/merge_baseline.py --selfcheck
run "$PY" scripts/catalog_drift.py --selfcheck
# Milestone dates are written from this script's arithmetic, and a rate that
# divides wrong writes a plausible date nobody can catch by eye; only this flag
# exercises the window, the weighting and the PATCH routing.
run "$PY" scripts/forge_tracking.py --selfcheck
# The PR metadata gate only ever runs in CI (040), so the tree's own check of it
# is this flag: a fake transport walks every branch — each issue defect, each fgj
# fix line, and the exempt path that must ask the forge nothing at all.
run "$PY" scripts/guardrails/check_pr_metadata.py --selfcheck
# The landing verbs decide from the forge's answers — behind, failed, ready — and a
# decision read wrong retries a refusal forever; only this flag walks the table.
run "$PY" scripts/forge_pr.py --selfcheck
# The orphan scans are heuristics over text, so the flag is where they are proved to
# judge anything at all: each scan is disabled in turn and the selfcheck must fail
# for that scan's own reason (D109).
run "$PY" scripts/guardrails/check_orphans.py --selfcheck
# The public surface is what the mirror publishes, modelled on filter-repo's own
# exclusion and substitution; a model that drifts from it passes a leak (D172).
run "$PY" scripts/guardrails/check_public_surface.py --selfcheck
# Prose is not exempt: 1,485 comment lines are under ratchet, and the design docs
# are the reference. Config and the domain-word allowlist live in .codespellrc.
if command -v codespell >/dev/null; then run codespell; else echo "FAIL codespell (uv tool install codespell)"; FAILED=$((FAILED+1)); fi
# binary_size and startup are the only two readers of target/dist/yi, and the
# fat-LTO build that writes it is minutes, so this is where it is paid for
# (D91): one branch decides whether either gate runs, and the build lives
# inside it. D70: the baseline is a macOS arm64 byte count, so any other
# target measures a different binary against it. D68: wall clock on a shared
# runner is noise against a 5 ms budget. --fast skips both because one needs
# the build and hyperfine is most of the other. Announced, never silent (9).
if [ "$FAST" -eq 1 ]; then
  echo "skip binary_size (--fast: needs a dist build)"
  echo "skip startup (--fast: hyperfine is most of the run)"
elif [ -n "${CI:-}" ]; then
  echo "skip binary_size (D70: baseline is macOS arm64; CI is another target)"
  echo "skip startup (D68: a shared runner cannot measure a 5 ms budget)"
  echo "skip build-dist (D91: nothing on CI reads target/dist/yi)"
elif scripts/build_dist.sh; then
  run "$PY" scripts/guardrails/check_binary_size.py
  run "$PY" scripts/guardrails/check_startup.py
else
  # A stale target/dist/yi from an earlier build would pass both gates while
  # measuring code nobody wrote, so a failed build fails them instead.
  echo "FAIL dist build (binary_size and startup have nothing to measure)"
  FAILED=$((FAILED + 1))
fi
# Growth is priced once per version, in the changelog row a landing writes last,
# so asking it of every commit inside that landing only teaches people to ignore
# it; --fast is the pre-commit hook and the full run is the push.
if [ "$FAST" -eq 1 ]; then
  echo "skip growth (--fast: a version's growth is priced at the push, not per commit)"
else
  run "$PY" scripts/guardrails/check_growth.py
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
