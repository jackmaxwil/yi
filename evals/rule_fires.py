#!/usr/bin/env python3
"""Labelled haystack lanes vs what D54's matcher can see today.

Zero spend. A cassette is JSONL of {lane, haystack, label, needle?}.
Lanes: args (tool JSON, pre-gate), result, error (is_error bodies), text
(assistant prose). The current engine fires on args and text only. This script does not load
`RuleEngine`; it models those scopes as a substring haystack.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

LANES = ("args", "result", "error", "text")
CURRENT = frozenset(("args", "text"))


def load_events(path: Path) -> list[dict]:
    events = []
    for line in path.read_text().splitlines():
        if line.strip():
            events.append(json.loads(line))
    return events


def needle_of(event: dict, default: str) -> str:
    return event.get("needle") or default


def fires(haystack: str, needle: str) -> bool:
    return bool(needle) and needle in haystack


def measure(path: Path, needle: str = "E0502") -> dict:
    events = load_events(path)
    rows = []
    for event in events:
        lane = event["lane"]
        n = needle_of(event, needle)
        hit = fires(event["haystack"], n)
        want = event["label"] == "should"
        rows.append(
            {
                "lane": lane,
                "label": event["label"],
                "hit": hit,
                "current": hit if lane in CURRENT else False,
                "oracle": hit,
                "want": want,
            }
        )
    should = [r for r in rows if r["want"]]
    should_not = [r for r in rows if not r["want"]]

    def rate(subset, key):
        if not subset:
            return None
        tp = sum(1 for r in subset if r[key])
        return tp / len(subset)

    fp = [r for r in should_not if r["oracle"]]
    return {
        "events": len(rows),
        "should": len(should),
        "should_not": len(should_not),
        "recall_current": rate(should, "current"),
        "recall_oracle": rate(should, "oracle"),
        "fp_oracle": rate(should_not, "oracle"),
        "fp_current": rate(should_not, "current"),
        "gap": [
            r["lane"]
            for r in should
            if r["oracle"] and not r["current"]
        ],
        "comment_fp": sum(
            1
            for r in rows
            if r["lane"] == "text" and not r["want"] and r["oracle"]
        ),
        "false_oracle": len(fp),
    }


def main() -> None:
    path = Path(sys.argv[1] if len(sys.argv) > 1 else "evals/fixtures/rules/lanes.jsonl")
    report = measure(path)
    json.dump(report, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
