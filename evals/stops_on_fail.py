#!/usr/bin/env python3
"""Stops a run took while its last ipython cell printed a FAIL line (EXP-B3c).

    python3 evals/stops_on_fail.py <session.jsonl or run dir>...

One JSON line per root session (a child's file, whose header names a parent, is skipped):
its task, `stops` and `stopsOnFail`. A stop ends a turn: an accepted `answer`, else a reply
that stopped with nothing after it but host notes. A FAIL line is unattended.md's verifier shape, `FAIL <requirement>` at the
start of a line of the cell's stdout. Stdlib only.
"""
import json, re, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT))

import axes  # noqa: E402

FAIL_LINE = re.compile(r"^\s*FAIL\b", re.M)


def cell_stdout(message):
    details = message.get("details")
    if isinstance(details, dict) and isinstance(details.get("stdout"), str):
        return details["stdout"]
    return "\n".join(part.get("text", "") for part in message.get("content") or () if isinstance(part, dict))


def count(path):
    """(stops, stops taken while the last cell so far printed FAIL) of one session file.

    A reply that stops stands only if the model does nothing more before the next typed message
    or the end of the file: a stop the host sent back, as the todo intercept does, is not taken."""
    last, stood, pending, ended = "", [], None, False
    for line in Path(path).read_text(errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        message = entry.get("message") if entry.get("type") == "message" else None
        if not isinstance(message, dict):
            continue
        role, tool = message.get("role"), message.get("toolName")
        if role == "user" and not isinstance(message.get("attribution"), dict):
            stood += [] if pending is None else [pending]
            pending, ended = None, False
            continue
        if role == "toolResult" and tool == "ipython":
            last = cell_stdout(message)
        if ended or role not in ("assistant", "toolResult"):
            continue
        if role == "toolResult" and tool == "answer" and not message.get("isError"):
            stood.append(bool(FAIL_LINE.search(last)))
            pending, ended = None, True
        else:
            pending = bool(FAIL_LINE.search(last)) if message.get("stopReason") == "stop" else None
    stood += [] if pending is None else [pending]
    return len(stood), sum(stood)


def is_root(path):
    with Path(path).open(errors="replace") as handle:
        return not json.loads(handle.readline()).get("parentSessionId")


def rows(paths):
    for given in map(Path, paths):
        root = given if given.is_dir() else given.parent
        for path in axes.scored_sessions(given) if given.is_dir() else [given]:
            if not is_root(path):
                continue
            kind, trial = axes.context_of(path, root)
            task = (axes.harbor_context(trial)["task"] or "").rsplit("/", 1)[-1] if kind == "harbor" else trial.name
            stops, on_fail = count(path)
            yield {"task": task, "trial": trial.name, "session": str(path), "stops": stops, "stopsOnFail": on_fail}


def main(argv=None):
    paths = sys.argv[1:] if argv is None else argv
    if not paths:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    for row in rows(paths):
        print(json.dumps(row))
    return 0


if __name__ == "__main__":
    sys.exit(main())
