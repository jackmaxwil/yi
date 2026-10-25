#!/usr/bin/env python3
"""Git merge driver for the ratchet baselines: two branches that both ratcheted never
conflict again. A ceiling merges to the looser side and `just ratchet` re-measures and
shrinks it in its own commit; the schema lock merges to the union, since shapes only grow.

    .gitattributes: scripts/guardrails/baselines/*.json merge=baseline
                    scripts/guardrails/baselines/schemas.lock merge=baseline
    git config merge.baseline.driver "python3 scripts/merge_baseline.py %O %A %B"
"""
import json, pathlib, sys


def atom(value):
    """A tool-surface lock value is a bare hash or `{hash, text}`: one unit, known by its hash."""
    return value["hash"] if isinstance(value, dict) and "hash" in value else value


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
            elif isinstance(a, dict) and isinstance(b, dict) and "hash" not in a:
                out[key] = merge(a, b, base.get(key))
            elif a is None or (atom(a) == atom(base.get(key)) and b is not None):
                out[key] = b
            else:
                out[key] = a
        return out
    return ours if ours is not None else theirs


def layout(value):
    """The one byte layout of a baseline, which every `--update` writer emits too.
    Incident: the writers used indent 1 and one line, so each merge or re-measure rewrote 700 lines."""
    return json.dumps(value, indent=2, sort_keys=isinstance(value, dict) and "max_bytes" not in value) + "\n"


def drive(base, ours, theirs):
    a = json.loads(pathlib.Path(ours).read_text())
    b = json.loads(pathlib.Path(theirs).read_text())
    o = pathlib.Path(base).read_text().strip()
    merged = merge(a, b, json.loads(o) if o else None)
    pathlib.Path(ours).write_text(layout(merged))
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
    entry = lambda hash_: {"hash": hash_, "text": f"text of {hash_}"}
    mixed = {"ours entry, theirs bare": (entry("old"), "new", "old", "new"), "both entries, bare base": (entry("old"), entry("new"), "old", entry("new")),
             "ours moved, theirs entry": ("new", entry("old"), "old", "new")}
    for name, (a, b, base, want) in mixed.items():
        assert merge({"tool:bash": a}, {"tool:bash": b}, {"tool:bash": base}) == {"tool:bash": want}, f"lock shapes meet: {name}"
    assert merge({"version": "0.142.0", "loc": 73264}, {"version": "0.142.0", "loc": 73264}) == {"version": "0.142.0", "loc": 73264}
    baselines = pathlib.Path(__file__).resolve().parent / "guardrails/baselines"
    drift = [p.name for p in sorted(baselines.glob("*.json")) + [baselines / "schemas.lock"]
             if p.read_text() != layout(json.loads(p.read_text()))]
    assert not drift, f"baselines not in the merge driver's layout, so the next merge rewrites them whole: {drift}"
    print("ok   merge_baseline selfcheck")


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        selfcheck()
        sys.exit(0)
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(drive(*sys.argv[1:4]))
