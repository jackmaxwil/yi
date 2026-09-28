"""Read back one cache probe: two `yi ask --json` transcripts and `yi stats`.

Prints one line per turn and the session's cache block. Exit 2 when the warm
turn read nothing from the cache or any turn priced as D79 `usage.unknown`; a
probe that cannot tell a hit from a miss must not read as a hit.

    python3 evals/drivers/cache_probe.py pin <root>

reads `<root>/<upstream>/<sample>/t{1,2}.jsonl` and prints the pin verdict as JSON (N4,
docs/plans/2026-09-26-self-improvement-evals.md 6.4): the cheapest upstream whose every
warm turn hit at least PIN_HIT and that never answered 429; with none, the best mean hit
rate with fallbacks allowed, and the verdict says so.
"""

import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "adapters"))
from yi_usage import parse_events  # noqa: E402


def hit_rate(turn):
    whole = (turn["input"] or 0) + (turn["cacheRead"] or 0) + (turn["cacheWrite"] or 0)
    return (turn["cacheRead"] or 0) / whole if whole else 0.0


PIN_HIT = 0.9
# Incident (N4, 2026-09-27): on four of five upstreams the first session's warm turn read 0 and
# every later session's read 0.97; the first session on an upstream writes the cache the later
# ones read. That sample is a warm-up: it counts toward 429s and pricing, not toward the hit.
WARMUP = 1


def pin(probes, warmup=WARMUP):
    """`probes`: {upstream: [{"t2": parse_events row, "error": str or None}, ...]} in run order."""
    table = {}
    for upstream, samples in sorted(probes.items()):
        failed = [s for s in samples if s["error"] or not s["t2"]["nAssistantMessages"]
                  or s["t2"]["costUnknownTurns"]]
        samples = samples[warmup:] if len(samples) > warmup else samples
        priced = [s for s in samples if not s["error"] and s["t2"]["nAssistantMessages"]
                  and not s["t2"]["costUnknownTurns"]]
        hits = [hit_rate(s["t2"]) for s in priced]
        table[upstream] = {
            "samples": len(samples), "failed": len(failed),
            "hits": [round(h, 4) for h in hits],
            "meanHit": round(sum(hits) / len(hits), 4) if hits else 0.0,
            "meanCostUsd": round(sum(s["t2"]["costUsd"] or 0 for s in priced) / len(priced), 6) if priced else None,
            "qualifies": bool(priced) and not failed and min(hits) >= PIN_HIT,
        }
    good = [u for u, row in table.items() if row["qualifies"]]
    if good:
        chosen, fallbacks = min(good, key=lambda u: (table[u]["meanCostUsd"], u)), False
    else:
        chosen, fallbacks = max(table, key=lambda u: (table[u]["meanHit"], u)), True
    return {"pin": chosen, "allowFallbacks": fallbacks,
            "routing": {"order": [chosen], "allow_fallbacks": fallbacks}, "upstreams": table}


def read_probes(root):
    probes = {}
    for sample in sorted(Path(root).glob("*/*"), key=lambda p: (p.parent.name, int(p.name))):
        texts = [p.read_text(errors="replace") for p in sample.glob("t*.jsonl")]
        error = "HTTP 429" if any("429" in t for t in texts) else None
        probes.setdefault(sample.parent.name, []).append(
            {"t2": parse_events(sample / "t2.jsonl"), "error": error})
    return probes


def main(binary, session_dir):
    worst = 0
    for name in ("t1", "t2"):
        turn = parse_events(os.path.join(session_dir, name + ".jsonl"))
        if turn["nAssistantMessages"] == 0 or turn["costUnknownTurns"]:
            print(f"{name}: no priced assistant turn ({turn})")
            worst = 2
            continue
        print(
            f"{name}: input {turn['input']} cacheRead {turn['cacheRead']} "
            f"cacheWrite {turn['cacheWrite']} cost {turn['costUsd']} "
            f"hit {hit_rate(turn):.3f}"
        )
        if name == "t2" and not turn["cacheRead"]:
            print("t2: warm turn read nothing from the cache")
            worst = 2
    stats = subprocess.run(
        [binary, "stats", "--json", "--session-dir", session_dir],
        capture_output=True,
        text=True,
        check=False,
    )
    if stats.returncode == 0:
        row = json.loads(stats.stdout)
        print("stats:", json.dumps({"cache": row.get("cache"), "models": row.get("models")}))
    else:
        print("stats failed:", stats.stderr.strip())
        worst = max(worst, 2)
    return worst


if __name__ == "__main__":
    if sys.argv[1:2] == ["pin"]:
        print(json.dumps(pin(read_probes(sys.argv[2])), sort_keys=True))
        sys.exit(0)
    sys.exit(main(sys.argv[1], sys.argv[2]))
