#!/bin/sh
# Terminal-Bench v4 six-task subset (docs/plans/2026-09-06-tbv4-evals.md, S1).
# Preflights every precondition by NAME and refuses with the missing one; it
# never echoes a key value. Run from the worktree root.
set -u

# Pinned by digest (E13): leaderboard/src/leaderboard/core/hub.py:27 of the
# terminal-bench clone; `@latest` would move under a row.
DATASET="terminal-bench/terminal-bench@sha256:39d9f44b40420cde8fdcc087579c0d72a7e14fa3656d603c3f0d22fb35e27732"
TASKS="${TBV4_TASKS:-html-js-filter photonic-waveguide-routing music-harmony bun-sourcemap-leak foodstuff-beta-activity cargo-flight-dispatch}"
ATTEMPTS="${TBV4_ATTEMPTS:-1}"
CONCURRENCY="${TBV4_CONCURRENCY:-2}"
# One hour of the task's 28,800 s (plan §8, decision 2); the ceiling is the
# default, so no row on this ledger ever runs longer.
MULT="${TBV4_TIMEOUT_MULT:-0.125}"
MODEL="openrouter/z-ai/glm-5.3-flash"
BINARY="${EVAL_BINARY:-target/x86_64-unknown-linux-musl/dist/yi}"
SOFT_CAP="${TBV4_SOFT_CAP:-20}"
HARD_CAP="${TBV4_HARD_CAP:-25}"
RUNS_DIR="${TBV4_RUNS_DIR:-runs/tbv4}"

fail() { echo "refused: $1" >&2; exit 1; }

# The ceiling first: it needs nothing installed, so the selftest can see it red.
python3 - "$MULT" <<'PY' || fail "TBV4_TIMEOUT_MULT $MULT is above the 0.125 ceiling (one hour)"
import sys; sys.exit(0 if 0 < float(sys.argv[1]) <= 0.125 else 1)
PY
[ -n "${OPENROUTER_API_KEY:-}" ] || fail "OPENROUTER_API_KEY is unset"
docker info >/dev/null 2>&1 || fail "docker daemon is not running"
command -v harbor >/dev/null 2>&1 || fail "harbor is not installed (uv tool install harbor)"
[ -x "$BINARY" ] || fail "no musl binary at $BINARY (just package-musl <version>)"
# A cross binary cannot run on this host (justfile package-musl says so); the container's
# `yi --version` at install is the smoke, and the adapter refuses there.

export EVAL_BINARY="$BINARY"
export EVAL_SUITE_REV="${DATASET#*@}"
export EVAL_TIMEOUT_MULT="$MULT"
mkdir -p "$RUNS_DIR"
stopped=0
done_tasks=0
for task in $TASKS; do
    # Incident: the TB2.1 driver read `runs/` while harbor wrote `jobs/`, so the
    # probe summed nothing and the cap never tripped. The probe reads the
    # directory harbor is told to write, and after each task it must find one
    # more transcript than before, or the spend is unmeasurable.
    spent=$(python3 evals/drivers/tb21_cost.py "$RUNS_DIR" --soft "$SOFT_CAP" --hard "$HARD_CAP" --min-files "$done_tasks")
    probe=$?
    [ "$probe" -eq 0 ] || { echo "not starting $task (probe exit $probe)" >&2; stopped=2; break; }
    echo "== $task (spent \$$spent, multiplier $MULT)"
    PYTHONPATH=evals/adapters harbor run \
        --agent yi_harbor.agent:Yi \
        -d "$DATASET" \
        -i "terminal-bench/$task" \
        --model "$MODEL" \
        -k "$ATTEMPTS" \
        -n "$CONCURRENCY" \
        --agent-timeout-multiplier "$MULT" \
        -o "$RUNS_DIR"
    done_tasks=$((done_tasks + ATTEMPTS))
done

echo "== totals"
python3 evals/drivers/tb21_cost.py "$RUNS_DIR" --min-files "$done_tasks" || stopped=2
exit "$stopped"
