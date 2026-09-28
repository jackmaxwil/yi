#!/bin/sh
# Terminal-Bench v4 calibration sweep: one continuous harbor run over a task list,
# held by watch.py (per-trial stop, spend and wall caps, image pruner). A list of
# trials run as one job fills every slot until the wall; per-task runs wait on
# their slowest trial. Same preflight as tbv4_baseline.sh; run from the worktree root.
#
# Runner mode, the protocol evals/levers.py calls: `tbv4_sweep.sh --runner <overrides.json>
# <task>...` runs those tasks as one job under the stage and week caps drivers/trials.py
# computes, files one row per trial under evals/trials/$EVAL_RUN_ID.jsonl, and prints those
# rows on stdout; harbor's own output goes to stderr. `{}` overrides run the defaults.
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
RUNNER=""

fail() { echo "refused: $1" >&2; exit 1; }

if [ "${1:-}" = "--runner" ]; then
  [ $# -ge 3 ] || fail "--runner wants <overrides.json> <task>..."
  OVERRIDES=$2
  shift 2
  RUN_ID="${EVAL_RUN_ID:-}"
  [ -n "$RUN_ID" ] || fail "EVAL_RUN_ID is unset; the trial store files this call's rows under it"
  RUNNER=1
  LIST=$(mktemp)
  printf '%s\n' "$@" > "$LIST"
  RUNS_DIR="${TBV4_RUNS_ROOT:-$HOME/Development/yi-runs}/$RUN_ID/$(date +%Y%m%d-%H%M%S)-$$"
fi

python3 - "$MULT" <<'PY' || fail "TBV4_TIMEOUT_MULT $MULT is above the 0.125 ceiling (one hour)"
import sys; sys.exit(0 if 0 < float(sys.argv[1]) <= 0.125 else 1)
PY
[ -n "${OPENROUTER_API_KEY:-}" ] || fail "OPENROUTER_API_KEY is unset"
docker info >/dev/null 2>&1 || fail "docker daemon is not running"
command -v harbor >/dev/null 2>&1 || fail "harbor is not installed (uv tool install harbor)"
[ -x "$BINARY" ] || fail "no musl binary at $BINARY (just package-musl <version>)"
[ -s "$LIST" ] || fail "no task list at $LIST"

if [ -n "$RUNNER" ]; then
  [ -r "$OVERRIDES" ] || fail "no overrides file at $OVERRIDES"
  TRIALS=$(( $(wc -l < "$LIST") * ATTEMPTS ))
  HARD_CAP=$(python3 evals/drivers/trials.py caps --run-id "$RUN_ID" --tasks "$TRIALS") || exit 2
  if [ "$(tr -d ' \n\t' < "$OVERRIDES")" = "{}" ]; then
    unset YI_LEVERS
    ARM="${EVAL_ARM:-defaults}"
  else
    export YI_LEVERS="$OVERRIDES"
    ARM="${EVAL_ARM:-levers:$(shasum -a 256 "$OVERRIDES" | cut -c1-12)}"
  fi
fi

set --
for task in $(cat "$LIST"); do set -- "$@" -i "terminal-bench/$task"; done
export EVAL_BINARY="$BINARY"
export EVAL_SUITE_REV="${DATASET#*@}"
export EVAL_TIMEOUT_MULT="$MULT"
export PYTHONPATH=evals/adapters
# Three trials share the host; a verifier timeout would score the agent's finished work 0.
if [ -z "$RUNNER" ]; then
  exec python3 evals/drivers/watch.py --runs "$RUNS_DIR" --hard "$HARD_CAP" --wall "$WALL" -- \
      harbor run --agent yi_harbor.agent:Yi -d "$DATASET" "$@" \
      --model "$MODEL" -k "$ATTEMPTS" -n "$CONCURRENCY" \
      --agent-timeout-multiplier "$MULT" --verifier-timeout-multiplier 1.5 -o "$RUNS_DIR"
fi
python3 evals/drivers/watch.py --runs "$RUNS_DIR" --hard "$HARD_CAP" --wall "$WALL" -- \
    harbor run --agent yi_harbor.agent:Yi -d "$DATASET" "$@" \
    --model "$MODEL" -k "$ATTEMPTS" -n "$CONCURRENCY" \
    --agent-timeout-multiplier "$MULT" --verifier-timeout-multiplier 1.5 -o "$RUNS_DIR" >&2
code=$?
# N1: a Docker Hub timeout killed a trial's environment before its agent started, which is not
# a result. A trial that left no session ran no agent, so its task runs once more; a trial that
# did run is never rerun, whatever it raised (harbor's RuntimeError also reports an artifact
# the agent failed to write, which is the agent's result).
AGAIN=$(python3 evals/drivers/trials.py unstarted "$RUNS_DIR")
if [ "$code" = 0 ] && [ -n "$AGAIN" ]; then
  echo "rerunning tasks whose environment never started: $AGAIN" >&2
  set --
  for task in $AGAIN; do set -- "$@" -i "terminal-bench/$task"; done
  python3 evals/drivers/watch.py --runs "$RUNS_DIR-again" --hard "$HARD_CAP" --wall "$WALL" -- \
      harbor run --agent yi_harbor.agent:Yi -d "$DATASET" "$@" \
      --model "$MODEL" -k 1 -n "$CONCURRENCY" \
      --agent-timeout-multiplier "$MULT" --verifier-timeout-multiplier 1.5 -o "$RUNS_DIR-again" >&2
  code=$?
  python3 evals/drivers/trials.py rows "$RUNS_DIR-again" --run-id "$RUN_ID" --arm "$ARM" || true
fi
python3 evals/drivers/trials.py rows "$RUNS_DIR" --run-id "$RUN_ID" --arm "$ARM" || exit 2
exit $code
