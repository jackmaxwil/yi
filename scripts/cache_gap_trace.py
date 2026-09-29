#!/usr/bin/env python3
"""Reduce yi session files to the cache replay's gap-and-size trace (D315).

One output line per session with two or more real requests: a JSON list of
[start_s, prompt] pairs, where start_s is the request's start in whole seconds
since the session's first request and prompt is input + cache read + cache
write. Nothing else is kept: no ids, content, models, costs or absolute times.
A request's start is its entry's timestamp less the `elapsed_ms` the loop
records (#873); a record from before that carries its end.

    python3 scripts/cache_gap_trace.py ~/.yi/sessions > trace.jsonl
"""

import json
import pathlib
import sys


def requests(path):
    for line in path.read_text(errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        message = entry.get("message") or {}
        if (
            entry.get("kind") != "entry"
            or entry.get("lane", "main") != "main"
            or message.get("role") != "assistant"
            or message.get("provider") == "faux"
        ):
            continue
        usage = message.get("usage") or {}
        prompt = sum(usage.get(key, 0) for key in ("input", "cacheRead", "cacheWrite"))
        elapsed = next(
            (
                (note.get("details") or {}).get("elapsed_ms", 0)
                for note in message.get("diagnostics") or []
                if note.get("type") == "cache"
            ),
            0,
        )
        if prompt > 0:
            yield entry["timestamp"] - elapsed, prompt


def main(root):
    for path in sorted(pathlib.Path(root).glob("*/*.jsonl")):
        trace = list(requests(path))
        if len(trace) > 1:
            first = trace[0][0]
            print(json.dumps([[round((at - first) / 1000), p] for at, p in trace], separators=(",", ":")))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path.home() / ".yi" / "sessions")
