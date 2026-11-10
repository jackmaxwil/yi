#!/usr/bin/env bash
# Aggregator = the gate entrypoint itself (D31; a surveyed aggregator was referenced by
# nothing). Ordered most-legible-failure-first. Prints one debt total at the end.
# `--fast` drops the gates that need a build (dist binary, startup, rustdoc, the two
# cargo tools) so pre-commit stays quick; pre-push runs the whole thing. `--quiet` prints only
# the failing gates' output, the `skip` rows (a skip is announced, never silent) and the summary line.
set -u
cd "$(dirname "$0")/../.."
FAST=0 QUIET=0
for arg in "$@"; do
  case "$arg" in --fast) FAST=1 ;; --quiet) QUIET=1 ;; esac
done
PY=$(scripts/py.sh) || exit 1
FAILED=0
# Every gate reads the tree and none writes it, so they all run at once; `drain` prints each
# one's output in launch order, so the report still reads most-legible-failure-first.
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
N=0
run() { N=$((N + 1)); printf '%s\n' "$*" > "$OUT/$N.cmd"; { "$@"; echo $? > "$OUT/$N.rc"; } > "$OUT/$N.out" 2>&1 & }
# A gate that failed without printing a FAIL row is named, with its own output indented under the row: that
# tail is the detail scripts/pr_autofix.py failures() hands the repair turn beside it.
named() {
  printf 'FAIL %s (exit %s, printed no FAIL line)\n' "$3" "$2"
  sed 's/^/  /' "$1"
}
drain() {
  wait
  local i rc
  for i in $(seq 1 "$N"); do
    rc=$(cat "$OUT/$i.rc")
    if [ "$rc" = 0 ]; then
      if [ "$QUIET" = 1 ]; then grep '^skip ' "$OUT/$i.out"; else cat "$OUT/$i.out"; fi
    else
      if grep -q '^FAIL' "$OUT/$i.out"; then
        cat "$OUT/$i.out"
      else
        named "$OUT/$i.out" "$rc" "$(cat "$OUT/$i.cmd")"
      fi
      FAILED=$((FAILED + 1))
    fi
  done
}
# The dist gate runs after drain, on a quiet tree, so it collects like drain's and reports through it.
after() {
  N=$((N + 1))
  printf '%s\n' "$*" > "$OUT/$N.cmd"
  { "$@"; echo $? > "$OUT/$N.rc"; } > "$OUT/$N.out" 2>&1
  local rc
  rc=$(cat "$OUT/$N.rc")
  if [ "$rc" = 0 ]; then
    [ "$QUIET" = 1 ] || cat "$OUT/$N.out"
  elif grep -q '^FAIL' "$OUT/$N.out"; then
    cat "$OUT/$N.out"
  else
    named "$OUT/$N.out" "$rc" "$*"
  fi
  return "$rc"
}
public_mirror() { [ ! -d "$1/scripts/mirror" ]; }

# Only this flag proves --quiet keeps a green gate's skip row while dropping the rest of its output.
if [ "${1:-}" = --selfcheck ]; then
  N=3 QUIET=1
  mkdir -p "$OUT/source/scripts/mirror"
  if ! public_mirror "$OUT/public" || public_mirror "$OUT/source"; then
    echo "FAIL public mirror detection"
    exit 1
  fi
  printf 'skip deny (advisory database unreachable)\nnoise\n' > "$OUT/1.out"; echo 0 > "$OUT/1.rc"
  printf 'only noise\n' > "$OUT/2.out"; echo 0 > "$OUT/2.rc"
  printf 'FAIL x\nwhy\n' > "$OUT/3.out"; echo 1 > "$OUT/3.rc"; echo 'check_x.py' > "$OUT/3.cmd"
  [ "$(drain)" = "$(printf 'skip deny (advisory database unreachable)\nFAIL x\nwhy')" ] || { echo "FAIL quiet drain keeps skip rows and failures only"; exit 1; }
  N=4
  : > "$OUT/4.out"; echo 137 > "$OUT/4.rc"; echo 'check_y.py --selfcheck' > "$OUT/4.cmd"
  [ "$(drain)" = "$(printf 'skip deny (advisory database unreachable)\nFAIL x\nwhy\nFAIL check_y.py --selfcheck (exit 137, printed no FAIL line)')" ] || { echo "FAIL a gate killed before it printed is named"; exit 1; }
  N=1
  printf 'a traceback\nerror: boom\n' > "$OUT/1.out"; echo 1 > "$OUT/1.rc"; echo 'check_z.py' > "$OUT/1.cmd"
  [ "$(drain)" = "$(printf 'FAIL check_z.py (exit 1, printed no FAIL line)\n  a traceback\n  error: boom')" ] || { echo "FAIL a gate with no FAIL row is named and keeps its output under the row"; exit 1; }
  N=0
  [ "$(after false)" = "FAIL false (exit 1, printed no FAIL line)" ] || { echo "FAIL a gate after drain that printed nothing is named"; exit 1; }
  echo "ok guardrails quiet selfcheck"; exit 0
fi

run "$PY" scripts/guardrails/check_manifests.py
run "$PY" scripts/guardrails/check_boundaries.py
run "$PY" scripts/guardrails/check_filenames.py
run "$PY" scripts/guardrails/check_workflows.py
run "$PY" scripts/guardrails/check_agents_md.py
run "$PY" scripts/guardrails/check_skills_sync.py
run "$PY" scripts/guardrails/check_skills_sync.py --selfcheck
run "$PY" scripts/guardrails/check_commit_style.py
run "$PY" scripts/guardrails/check_glob_reexport.py
run "$PY" scripts/guardrails/check_orphans.py
run "$PY" scripts/guardrails/check_panic.py
run "$PY" scripts/guardrails/check_host_git.py
run "$PY" scripts/guardrails/check_changes.py
run "$PY" scripts/guardrails/check_intent.py
run "$PY" scripts/guardrails/check_comments.py
run "$PY" scripts/guardrails/check_file_size.py
run "$PY" scripts/guardrails/check_crate_size.py
run "$PY" scripts/guardrails/check_fn_size.py
run "$PY" scripts/guardrails/check_schemas_lock.py
run "$PY" scripts/guardrails/check_env_surface.py
run "$PY" scripts/guardrails/check_duplication.py
run "$PY" scripts/guardrails/check_test_size.py
run "$PY" scripts/guardrails/check_test_tiers.py
run "$PY" scripts/guardrails/check_test_home.py
run "$PY" scripts/guardrails/check_test_home.py --selfcheck
run "$PY" scripts/guardrails/check_ledger_journeys.py
run "$PY" scripts/guardrails/check_ledger_journeys.py --selfcheck
run "$PY" scripts/guardrails/check_blob_size.py
run "$PY" scripts/guardrails/check_public_surface.py
run "$PY" scripts/guardrails/check_deps_budget.py
run "$PY" scripts/guardrails/check_request_budget.py
run "$PY" scripts/guardrails/check_behavior.py
if [ "$FAST" -eq 1 ]; then
  run echo "skip scan (--fast: pinned external scanners run in the full lane)"
  run echo "skip scan selfcheck (--fast: pinned external scanners run in the full lane)"
elif [ ! -d scripts/scan ] && public_mirror .; then
  run echo "skip scan (public mirror omits scripts/scan; run full check in unfiltered source checkout)"
  run echo "skip scan selfcheck (public mirror omits scripts/scan; run full check in unfiltered source checkout)"
else
  run "$PY" scripts/guardrails/check_scan.py
  run "$PY" scripts/guardrails/check_scan.py --selfcheck
fi
run "$PY" scripts/guardrails/check_prompt_examples.py
run env PYTHONPATH=python/yi_runtime/src "$PY" -m unittest discover -q -s python/yi_runtime/tests
run "$PY" -m unittest discover -q -s adapters
run "$PY" evals/selftest.py
# A prompt's rules are text, so a rewording that drops one passes every other gate; the
# scorer's cases are hand-counted, which is where it is proved to judge anything at all (#1862).
run "$PY" evals/prose.py --gate
run "$PY" evals/prose.py --selfcheck
# The mined artifacts carry session text, so §10's planted-fake redaction proof is a
# gate, not a habit: its only executable check is this flag.
run "$PY" skills/yi/session-mining/extract.py --selfcheck
# The PR narrative's net-src number is what the growth budget is argued against, and
# a path in the wrong bucket misprices it silently; only this flag exercises the split.
run "$PY" scripts/pr_body.py --selfcheck
run "$PY" scripts/test_sign.py
# A checked-in filter over test names rots silently, so only this flag proves a stale row is
# refused; it lists no workspace, so the fast lane runs it too.
run "$PY" scripts/slow_tests.py --selfcheck
# The snap diff an agent reads is parsed from casts, checked against Yi's own screen dumps.
run "$PY" scripts/tui_see.py --selfcheck
# The monkey's seed must replay byte for byte or a failure it reports cannot be reproduced.
run "$PY" scripts/tui_monkey.py --selfcheck
# The frame audit is only evidence if each of its checks can fail; only this flag trips them.
run "$PY" scripts/tui_pty.py --selfcheck
# Only this flag proves the skip row cannot swallow a real advisory.
run "$PY" scripts/guardrails/check_deny.py --selfcheck
run bash "$0" --selfcheck
# The tool-surface lock is equality over hashes, so a comparison that drifted would
# pass every change or refuse every run; only this flag walks the delta cases.
run "$PY" scripts/guardrails/check_request_budget.py --selfcheck
run "$PY" scripts/guardrails/check_fn_size.py --selfcheck
# Which src lines are test code decides the production gates and the test budget alike.
run "$PY" scripts/guardrails/check_test_size.py --selfcheck
run "$PY" scripts/guardrails/check_file_size.py --selfcheck
# Both read the real tree, which holds no defect of either kind, so only these flags plant one.
run "$PY" scripts/guardrails/check_boundaries.py --selfcheck
run "$PY" scripts/guardrails/check_env_surface.py --selfcheck
# The prose windows scan zero hits today, so only this flag proves a planted copy still fails.
run "$PY" scripts/guardrails/check_duplication.py --selfcheck
# The size-report comment is upserted by marker, and a marker that stops matching
# posts a duplicate rather than failing; only this flag exercises the routing.
run "$PY" scripts/forgejo_pr_comment.py --selfcheck
run "$PY" scripts/guardrails/check_workflows.py --selfcheck
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
# Change files are the one place a branch records a change; the format and the view-edit
# refusals are rules over text, so only this flag proves each refusal fires.
run "$PY" scripts/guardrails/check_changes.py --selfcheck
run "$PY" scripts/guardrails/check_intent.py --selfcheck
# The recorder numbers versions and decisions on main, where a wrong order is a renumbering
# nobody can undo; only this flag runs it over real change files in their real landing order.
if [ "$FAST" -eq 1 ]; then
  run echo "skip record selfcheck (--fast: replays two record branches and ten gate runs)"
else
  run "$PY" scripts/record_changes.py --selfcheck
fi
# A baseline edit that rides beside code is how growth hid before the ratchets had their own
# commits; the deletion carve-out is a rule over file statuses, so only this flag proves it.
run "$PY" scripts/guardrails/check_commit_style.py --selfcheck
run "$PY" scripts/pr_queue.py --selfcheck
# The landing verbs decide from the forge's answers — behind, failed, ready — and a
# decision read wrong retries a refusal forever; only this flag walks the table.
run "$PY" scripts/forge_pr.py --selfcheck
# A review round is posted from model answers the script cannot trust, so the host's own
# decisions are the gate: the quote check, the refute tally, the marker's author, the
# delta range and the fixer's wall; only this flag walks them without a forge or a model.
run "$PY" scripts/pr_review.py --selfcheck
run "$PY" scripts/intake/bridge.py --selfcheck
# The autofixer pushes to people's branches, so its table, its wall and its marker check are
# proved here on a scratch merge with a stand-in model, never a forge.
run "$PY" scripts/pr_autofix.py --selfcheck
run "$PY" scripts/fixer_guard.py --selfcheck
run "$PY" scripts/owners.py --selfcheck
run "$PY" scripts/checkout_guard.py --selfcheck
# Every bot comment's spend is read back from its meta line, so the parse and the totals are proved here.
run "$PY" scripts/bot_meter.py --selfcheck
# The orphan scans are heuristics over text, so the flag is where they are proved to
# judge anything at all: each scan is disabled in turn and the selfcheck must fail
# for that scan's own reason (D109).
run "$PY" scripts/guardrails/check_orphans.py --selfcheck
run "$PY" scripts/guardrails/check_host_git.py --selfcheck
# The public surface is what the mirror publishes, modelled on filter-repo's own
# exclusion and substitution; a model that drifts from it passes a leak (D172).
run "$PY" scripts/guardrails/check_public_surface.py --selfcheck
# The GitHub release's scan (scripts/mirror/publish.sh), refuted with each check off.
if public_mirror .; then
  run echo "skip mirror archive scan (public mirror omits scripts/mirror; run full check in unfiltered source checkout)"
else
  run "$PY" -I scripts/mirror/scan.py --selfcheck
fi
# Prose is not exempt: check_comments.py ratchets the comment volume and prints the count, and
# the design docs set the spelling. Config and the domain-word allowlist live in .codespellrc.
if command -v codespell >/dev/null; then run codespell; else run sh -c 'echo "FAIL codespell (uv tool install codespell)"; exit 1'; fi
# startup is the only reader of target/dist/yi, and the fat-LTO build that writes it is minutes,
# so this is where it is paid for (D91). CI never times startup on a shared runner (D68);
# --fast skips the build and hyperfine. The build runs beside the other gates, then startup
# runs after `drain` on a quiet tree. Skips are announced, never silent (9).
DIST=0
if [ "$FAST" -eq 1 ]; then
  run echo "skip startup (--fast: hyperfine is most of the run)"
elif [ -n "${CI:-}" ]; then
  run echo "skip startup (D68: a shared runner cannot measure a 6 ms budget)"
  run echo "skip build-dist (nothing on a CI host reads target/dist/yi)"
else
  run scripts/build_dist.sh
  DIST=$N
fi
# Growth is priced once per branch, in the change file a landing writes last,
# so asking it of every commit inside that landing only teaches people to ignore
# it; --fast is the pre-commit hook and the full run is the push.
if [ "$FAST" -eq 1 ]; then
  run echo "skip growth (--fast: a branch's growth is priced at the push, not per commit)"
else
  run "$PY" scripts/guardrails/check_growth.py
fi

if [ "$FAST" -eq 0 ]; then
  run cargo doc --workspace --no-deps --document-private-items -q

  if command -v cargo-machete >/dev/null; then run cargo machete crates; else run sh -c 'echo "FAIL machete (cargo install cargo-machete)"; exit 1'; fi
  if command -v cargo-deny >/dev/null; then run "$PY" scripts/guardrails/check_deny.py; else run sh -c 'echo "FAIL deny (cargo install cargo-deny)"; exit 1'; fi
else
  run echo "skip doc/machete/deny (--fast)"
fi
drain

if [ "$DIST" -gt 0 ]; then
  # A stand-in binary, so the startup gate is proved able to fail on each entry before it judges.
  [ -n "${CI:-}" ] || after "$PY" scripts/guardrails/check_startup.py --selfcheck || FAILED=$((FAILED + 1))
  if [ "$(cat "$OUT/$DIST.rc")" = 0 ]; then
    [ -n "${CI:-}" ] || after "$PY" scripts/guardrails/check_startup.py || FAILED=$((FAILED + 1))
  else
    # A stale target/dist/yi from an earlier build would pass startup while
    # measuring code nobody wrote, so a failed build fails it instead.
    echo "FAIL dist build (startup has nothing to measure)"
  fi
fi

[ "$QUIET" = 1 ] || echo "----"
if [ "$FAILED" -eq 0 ]; then echo "guardrails: all green"; else echo "guardrails: $FAILED failing"; exit 1; fi
