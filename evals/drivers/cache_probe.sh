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

# N4 pin mode (design 6.4): with CACHE_PROBE_UPSTREAMS set, the first model is probed
# CACHE_PROBE_SAMPLES times per OpenRouter upstream, each sample in its own HOME whose config
# pins that upstream (`routing` {"order": [u], "allow_fallbacks": false}) and skips the kernel
# prewarm, and `cache_probe.py pin` prints the verdict.
if [ -n "${CACHE_PROBE_UPSTREAMS:-}" ]; then
    model=${MODELS%% *}
    root="$RUNS_DIR/pin-$(echo "$model" | tr / _)"
    rm -rf "$root"
    for upstream in $CACHE_PROBE_UPSTREAMS; do
        n=1
        while [ "$n" -le "${CACHE_PROBE_SAMPLES:-3}" ]; do
            dir="$root/$upstream/$n"
            mkdir -p "$dir/home/.yi"
            printf '{"routing":{"order":["%s"],"allow_fallbacks":false},"kernel":{"prewarm":false}}' \
                "$upstream" > "$dir/home/.yi/config.json"
            HOME="$dir/home" "$BINARY" ask --json --yolo --model "$model" --session-dir "$dir" \
                "Reply with the single word ready." > "$dir/t1.jsonl" 2>&1
            HOME="$dir/home" "$BINARY" ask --json --yolo --continue --model "$model" --session-dir "$dir" \
                "Now reply with the single word again." > "$dir/t2.jsonl" 2>&1
            echo "== $upstream $n"
            n=$((n + 1))
        done
    done
    python3 evals/drivers/cache_probe.py pin "$root"
    exit $?
fi

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
