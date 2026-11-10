#!/bin/sh
# Terminal-Bench 2.1 six-task baseline (campaign 3, slice $25).
# Preflights every precondition by NAME and refuses with the missing one; it
# never echoes a key value. Run from the worktree root.
set -u

TASKS="${TB21_TASKS:-overfull-hbox fix-git regex-log db-wal-recovery password-recovery write-compressor}"
ATTEMPTS="${TB21_ATTEMPTS:-1}"
MODEL="openrouter/z-ai/glm-5.3-flash"
DATASET="terminal-bench/terminal-bench-2-1"
BINARY="${EVAL_BINARY:-target/x86_64-unknown-linux-musl/dist/yi}"
SOFT_CAP="${TB21_SOFT_CAP:-20}"
HARD_CAP="${TB21_HARD_CAP:-25}"
RUNS_DIR="${TB21_RUNS_DIR:-runs}"

fail() { echo "refused: $1" >&2; exit 1; }

[ -n "${OPENROUTER_API_KEY:-}" ] || fail "OPENROUTER_API_KEY is unset"
docker info >/dev/null 2>&1 || fail "docker daemon is not running"
command -v harbor >/dev/null 2>&1 || fail "harbor is not installed (pip install harbor)"
[ -x "$BINARY" ] || fail "no musl binary at $BINARY (just package-musl <version>)"
"$BINARY" --version >/dev/null 2>&1 || fail "$BINARY does not answer --version"

export EVAL_BINARY="$BINARY"
spent=0
# Incident: a cap stop used to exit 0, so a suite that burned its slice and a
# suite that finished were one exit code — the thing every gate here is read by.
stopped=0
for task in $TASKS; do
    # Incident: the comparison used to run here off the probe's stdout, so a
    # probe that failed left `spent` empty, `float('')` raised, and the empty
    # answer read as under-cap. The exit code is the gate now.
    spent=$(python3 evals/drivers/tb21_cost.py "$RUNS_DIR" --soft "$SOFT_CAP" --hard "$HARD_CAP")
    probe=$?
    [ "$probe" -eq 0 ] || { echo "not starting $task (probe exit $probe)" >&2; stopped=2; break; }
    echo "== $task (spent \$$spent)"
    PYTHONPATH=evals/adapters harbor run \
        --agent yi_harbor.agent:Yi \
        -d "$DATASET" \
        --task-name "$task" \
        --model "$MODEL" \
        --n-attempts "$ATTEMPTS"
done

echo "== totals"
python3 evals/drivers/tb21_cost.py "$RUNS_DIR"
exit "$stopped"
