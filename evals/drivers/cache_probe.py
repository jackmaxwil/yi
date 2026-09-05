"""Read back one cache probe: two `yi ask --json` transcripts and `yi stats`.

Prints one line per turn and the session's cache block. Exit 2 when the warm
turn read nothing from the cache or any turn priced as D79 `usage.unknown`; a
probe that cannot tell a hit from a miss must not read as a hit.
"""

import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "adapters"))
from yi_usage import parse_events  # noqa: E402


def hit_rate(turn):
    whole = (turn["input"] or 0) + (turn["cacheRead"] or 0) + (turn["cacheWrite"] or 0)
    return (turn["cacheRead"] or 0) / whole if whole else 0.0


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
    sys.exit(main(sys.argv[1], sys.argv[2]))
