#!/usr/bin/env python3
"""Git merge driver for the ratchet baselines: two branches that both ratcheted never
conflict again. A ceiling merges to the looser side and `just ratchet` re-measures and
shrinks it in its own commit; the schema lock merges to the union, since shapes only grow.

    .gitattributes: scripts/guardrails/baselines/*.json merge=baseline
                    scripts/guardrails/baselines/schemas.lock merge=baseline
    git config merge.baseline.driver "python3 scripts/merge_baseline.py %O %A %B"
"""
import json, pathlib, sys


def merge(ours, theirs, base=None):
    """Both sides parsed; the result is the one a re-measure can only shrink.
    Incident: a hash only main moved kept the branch's stale side, and #1110's fix hook refused it."""
    if isinstance(ours, dict) and isinstance(theirs, dict):
        base = base if isinstance(base, dict) else {}
        out = {}
        for key in sorted(set(ours) | set(theirs)):
            a, b = ours.get(key), theirs.get(key)
            if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool):
                out[key] = max(a, b)
            elif isinstance(a, dict) and isinstance(b, dict):
                out[key] = merge(a, b, base.get(key))
            elif a is None or (a == base.get(key) and b is not None):
                out[key] = b
            else:
                out[key] = a
        return out
    return ours if ours is not None else theirs


def drive(base, ours, theirs):
    a = json.loads(pathlib.Path(ours).read_text())
    b = json.loads(pathlib.Path(theirs).read_text())
    o = pathlib.Path(base).read_text().strip()
    merged = merge(a, b, json.loads(o) if o else None)
    text = json.dumps(merged, indent=2, sort_keys=isinstance(merged, dict) and "max_bytes" not in merged)
    pathlib.Path(ours).write_text(text + "\n")
    return 0


def selfcheck():
    assert merge({"max_bytes": 5_000}, {"max_bytes": 5_100}) == {"max_bytes": 5_100}, "the looser ceiling"
    assert merge({"orb": 1381, "tui": 11457}, {"orb": 1398, "tui": 11199}) == {"orb": 1398, "tui": 11457}
    assert merge({"volume": 2283, "over_cap": 0}, {"volume": 2256, "over_cap": 1}) == {"volume": 2283, "over_cap": 1}
    lock = merge({"a::X": "shape a", "c::Z": "old"}, {"b::Y": "shape b", "c::Z": "new"})
    assert lock == {"a::X": "shape a", "b::Y": "shape b", "c::Z": "old"}, "union, ours on a clash"
    surface = merge({"tool:bash": "old", "tool:plan": "ours"}, {"tool:bash": "main", "tool:plan": "old"},
                    {"tool:bash": "old", "tool:plan": "old"})
    assert surface == {"tool:bash": "main", "tool:plan": "ours"}, "a value only theirs moved is theirs"
    assert merge({"version": "0.142.0", "loc": 73264}, {"version": "0.142.0", "loc": 73264}) == {"version": "0.142.0", "loc": 73264}
    print("ok   merge_baseline selfcheck")


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        selfcheck()
        sys.exit(0)
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(drive(*sys.argv[1:4]))
