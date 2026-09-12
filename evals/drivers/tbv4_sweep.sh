#!/bin/sh
# Terminal-Bench v4 calibration sweep: one continuous harbor run over a task list,
# held by watch.py (per-trial stop, spend and wall caps, image pruner). A list of
# trials run as one job fills every slot until the wall; per-task runs wait on
# their slowest trial. Same preflight as tbv4_baseline.sh; run from the worktree root.
set -u

DATASET=$(grep '^DATASET=' evals/drivers/tbv4_baseline.sh | cut -d'"' -f2)
LIST="${TBV4_LIST:-evals/drivers/tbv4_sweep_tasks.txt}"
ATTEMPTS="${TBV4_ATTEMPTS:-1}"
CONCURRENCY="${TBV4_CONCURRENCY:-3}"
MULT="${TBV4_TIMEOUT_MULT:-0.125}"
MODEL="openrouter/z-ai/glm-5.3-flash"
BINARY="${EVAL_BINARY:-target/x86_64-unknown-linux-musl/dist/yi}"
HARD_CAP="${TBV4_HARD_CAP:-20}"
WALL="${TBV4_WALL:-27900}"
RUNS_DIR="${TBV4_RUNS_DIR:-runs/tbv4-sweep}"

fail() { echo "refused: $1" >&2; exit 1; }

python3 - "$MULT" <<'PY' || fail "TBV4_TIMEOUT_MULT $MULT is above the 0.125 ceiling (one hour)"
import sys; sys.exit(0 if 0 < float(sys.argv[1]) <= 0.125 else 1)
PY
[ -n "${OPENROUTER_API_KEY:-}" ] || fail "OPENROUTER_API_KEY is unset"
docker info >/dev/null 2>&1 || fail "docker daemon is not running"
command -v harbor >/dev/null 2>&1 || fail "harbor is not installed (uv tool install harbor)"
[ -x "$BINARY" ] || fail "no musl binary at $BINARY (just package-musl <version>)"
[ -s "$LIST" ] || fail "no task list at $LIST"

set --
for task in $(cat "$LIST"); do set -- "$@" -i "terminal-bench/$task"; done
export EVAL_BINARY="$BINARY"
export EVAL_SUITE_REV="${DATASET#*@}"
export EVAL_TIMEOUT_MULT="$MULT"
export PYTHONPATH=evals/adapters
# Three trials share the host; a verifier timeout would score the agent's finished work 0.
exec python3 evals/drivers/watch.py --runs "$RUNS_DIR" --hard "$HARD_CAP" --wall "$WALL" -- \
    harbor run --agent yi_harbor.agent:Yi -d "$DATASET" "$@" \
    --model "$MODEL" -k "$ATTEMPTS" -n "$CONCURRENCY" \
    --agent-timeout-multiplier "$MULT" --verifier-timeout-multiplier 1.5 -o "$RUNS_DIR"
