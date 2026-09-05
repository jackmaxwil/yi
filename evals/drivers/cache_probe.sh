#!/bin/sh
# Two-turn prompt-cache probe per OpenRouter model: a fresh session, then a
# `--continue` a few seconds later, and the usage each turn reports. Preflights
# every precondition by NAME and never echoes a key value. Run from the
# worktree root.
set -u

MODELS="${CACHE_PROBE_MODELS:-openrouter/z-ai/glm-5.3-flash openrouter/deepseek/deepseek-v4-flash-0731 openrouter/google/gemini-3.7-flash}"
BINARY="${EVAL_BINARY:-target/debug/yi}"
RUNS_DIR="${CACHE_PROBE_RUNS:-runs/cache-probe}"

fail() { echo "refused: $1" >&2; exit 1; }

[ -n "${OPENROUTER_API_KEY:-}" ] || fail "OPENROUTER_API_KEY is unset"
[ -x "$BINARY" ] || fail "no binary at $BINARY (cargo build -p yi-cli)"
"$BINARY" --version >/dev/null 2>&1 || fail "$BINARY does not answer --version"

worst=0
for model in $MODELS; do
    dir="$RUNS_DIR/$(echo "$model" | tr / _)"
    rm -rf "$dir"
    mkdir -p "$dir"
    echo "== $model"
    "$BINARY" ask --json --yolo --model "$model" --session-dir "$dir" \
        "Reply with the single word ready." > "$dir/t1.jsonl"
    "$BINARY" ask --json --yolo --continue --model "$model" --session-dir "$dir" \
        "Now reply with the single word again." > "$dir/t2.jsonl"
    python3 evals/drivers/cache_probe.py "$BINARY" "$dir"
    code=$?
    [ "$code" -gt "$worst" ] && worst=$code
done
exit "$worst"
