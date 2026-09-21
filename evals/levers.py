#!/usr/bin/env python3
"""The levers manifest, the floors and the two gates (D220; plan section 10).

A person runs this; nothing in the runtime calls it, and it starts no model.
`levers/levers.json` is the inventory of the kernel's constants, `levers/default.json`
the fixture `crates/runtime/tests/levers.rs` holds equal to the compiled defaults, and
`levers/floors.json` the capability floor per task class.

    python3 evals/levers.py --selfcheck

A trial row is the shape `evals/axes.py` scores: `task`, `reward`, `input`, `cacheRead`,
`output`, `costUsd`, `wallSec`. A lever change passes two gates (section 10.3): every
class stays within `tolerance` of its floor, and at least one of cost, tokens and wall
improves on the baseline; among the candidates that pass, only the nondominated survive.
"""
import argparse, json, pathlib, re, sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "evals/graph"))
from refine import Refused, names  # noqa: E402

LEVERS = ROOT / "evals/levers"
EFFICIENCY = ("costUsd", "tokens", "wallSec")
SCATTER = ROOT / "python/yi_runtime/src/yi/shapes.py"


def read(name):
    return json.loads((LEVERS / name).read_text())


def manifest():
    return {row["name"]: row for row in read("levers.json")["levers"]}


def selfcheck():
    """Every problem found, as one line each; an empty list is a pass."""
    rows, default, floors, split = read("levers.json")["levers"], read("default.json"), read("floors.json"), read("split.json")
    found = [f"{name} is listed twice" for name in {r["name"] for r in rows}
             if sum(1 for r in rows if r["name"] == name) > 1]
    if [r["name"] for r in rows] != list(default):
        found.append("levers.json and default.json name different levers, or in a different order")
    for row in rows:
        name, value = row["name"], default.get(row["name"])
        if value != row["default"]:
            found.append(f"{name}: the manifest says {row['default']}, default.json says {value}")
        if not all(isinstance(row[k], int) and not isinstance(row[k], bool) for k in ("default", "min", "max")):
            found.append(f"{name}: default, min and max are integers")
        elif not row["min"] <= row["default"] <= row["max"]:
            found.append(f"{name}: the default {row['default']} is outside {row['min']}..{row['max']}")
        if row["tunable"] is not True and not row.get("why"):
            found.append(f"{name}: a lever that is not tunable says why")
        if not (ROOT / row["home"]).is_file():
            found.append(f"{name}: no file {row['home']}")
    rounds = re.search(r"^SCATTER_MAX_ROUNDS = (\d+)$", SCATTER.read_text(), re.M)
    if not rounds or int(rounds[1]) != default.get("plan.scatter_rounds"):
        found.append("plan.scatter_rounds is not shapes.py's SCATTER_MAX_ROUNDS")
    groups = [task for group in ("development", "validation", "final") for task in split[group]]
    if len(groups) != len(set(groups)):
        found.append("split.json puts a task in two groups")
    for name, floor in floors.items():
        if set(floor) != {"pass_min", "reward_min", "tolerance", "tasks"}:
            found.append(f"floors.json: class {name} wants pass_min, reward_min, tolerance and tasks")
    return found


def classes(floors):
    return {task: name for name, floor in floors.items() for task in floor["tasks"]}


def measure(rows, floors):
    """Capability per class (pass rate over tasks, mean reward over trials) and the three
    efficiency sums over every trial, failed ones included. A task with no class is refused:
    a row that no floor covers would pass the first gate by not being looked at."""
    of = classes(floors)
    stray = sorted({row["task"] for row in rows} - set(of))
    if stray:
        raise Refused(f"task {stray[0]!r} has no class in floors.json")
    capability = {}
    for name in sorted({of[row["task"]] for row in rows}):
        mine = [row for row in rows if of[row["task"]] == name]
        tasks = {row["task"] for row in mine}
        solved = {row["task"] for row in mine if (row.get("reward") or 0) > 0}
        capability[name] = {"pass": len(solved) / len(tasks),
                            "reward": sum(row.get("reward") or 0 for row in mine) / len(mine)}
    tokens = sum((row.get(k) or 0) for row in rows for k in ("input", "cacheRead", "output"))
    return {"capability": capability, "tokens": tokens,
            "costUsd": sum(row.get("costUsd") or 0 for row in rows),
            "wallSec": sum(row.get("wallSec") or 0 for row in rows)}


def gate(baseline, candidate, floors):
    """None when both gates pass, else the reason with its class. The floor is judged first,
    so a candidate that got cheaper by failing is `below_floor:<class>`, never a saving."""
    for name, got in candidate["capability"].items():
        floor = floors[name]
        if got["pass"] < floor["pass_min"] - floor["tolerance"] \
                or got["reward"] < floor["reward_min"] - floor["tolerance"]:
            return f"below_floor:{name}"
    if set(baseline["capability"]) - set(candidate["capability"]):
        return "class_not_run:" + sorted(set(baseline["capability"]) - set(candidate["capability"]))[0]
    if not any(candidate[k] < baseline[k] for k in EFFICIENCY):
        return "no_efficiency_gain"
    return None


def dominates(one, other):
    """`one` is at least as good on every class and every cost, and better on something."""
    pairs = [(other[k], one[k]) for k in EFFICIENCY]
    for name in set(one["capability"]) | set(other["capability"]):
        mine, theirs = one["capability"].get(name), other["capability"].get(name)
        if mine is None or theirs is None:
            return False
        pairs += [(mine[k], theirs[k]) for k in ("pass", "reward")]
    return all(a >= b for a, b in pairs) and any(a > b for a, b in pairs)


def survivors(measured):
    """The nondominated among `{name: measure(...)}`; equal candidates both survive."""
    return sorted(name for name, mine in measured.items()
                  if not any(dominates(other, mine) for key, other in measured.items() if key != name))


def fit_rows(rows, split):
    """The rows a proposal may be fitted on. Refused whole when any row names a held-out
    task, by token as `refine.names` reads one: the fit never sees validation or final."""
    held_out = split["validation"] + split["final"]
    for number, row in enumerate(rows, 1):
        named = names(json.dumps(row), held_out)
        if named:
            raise Refused(f"row {number} names the held-out task {named[0]!r}")
    return rows


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selfcheck", action="store_true", help="hold the manifest, the fixture and the floors together")
    args = parser.parse_args(argv)
    if not args.selfcheck:
        parser.error("nothing to do; pass --selfcheck")
    found = selfcheck()
    for line in found:
        print(line, file=sys.stderr)
    print(f"levers: {len(manifest())} listed, {sum(1 for r in manifest().values() if r['tunable'])} tunable")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
