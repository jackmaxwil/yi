#!/usr/bin/env python3
"""The levers manifest, the floors and the two gates (D220; plan section 10).

A person runs this; nothing in the runtime calls it, and it starts no model.
`levers/levers.json` is the inventory of the kernel's constants, `levers/default.json`
the fixture `crates/runtime/tests/levers.rs` holds equal to the compiled defaults, and
`levers/floors.json` the capability floor per task class.

    python3 evals/levers.py --selfcheck
    python3 evals/levers.py compare --lever plan.width_max --value 4 --runner './score.sh'
    python3 evals/levers.py grid --knob plan.width_max=4,12 --knob todo.nudge_work=8,16 \\
        --runner './score.sh' --max-runs 30

The runner is the owner's: it is called as `<runner> <overrides.json> <task>...`, exports
`YI_LEVERS=<overrides.json>` to the runs it starts, and prints one JSON row per trial.

A trial row is the shape `evals/axes.py` scores: `task`, `reward`, `input`, `cacheRead`,
`output`, `costUsd`, `wallSec`. A lever change passes two gates (section 10.3): every
class stays within `tolerance` of its floor, and at least one of cost, tokens and wall
improves on the baseline; among the candidates that pass, only the nondominated survive.
"""
import argparse, itertools, json, math, pathlib, re, shlex, statistics, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "evals/graph"))
from refine import Refused, names  # noqa: E402

LEVERS = ROOT / "evals/levers"
EFFICIENCY = ("costUsd", "tokens", "wallSec")
MAX_KNOBS, MAX_RUNS, CONFIDENCE = 5, 40, 0.95
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


def candidate(overrides, listed):
    """Refused before anything is paid for, by the rules `Levers::load` applies to the same
    file: a lever the manifest lists, marked tunable, an integer inside its range."""
    for name, value in overrides.items():
        row = listed.get(name)
        if row is None:
            raise Refused(f"unknown lever {name}")
        if row["tunable"] is not True:
            raise Refused(f"{name} is not tunable: {row['why']}")
        if not isinstance(value, int) or isinstance(value, bool) or not row["min"] <= value <= row["max"]:
            raise Refused(f"{name} wants an integer in {row['min']}..{row['max']}, not {value!r}")
    return overrides


def interval(differences, confidence=CONFIDENCE):
    """The sign test inverted: an order-statistic interval for the median paired difference.
    It assumes only that the pairs are independent, which is what a handful of paired runs
    can support; a bootstrap over so few pairs is too narrow and a t interval assumes a
    normal spread that token counts do not have. `confidence` is what the interval really
    covers: with fewer than six pairs no interval reaches 0.95, and the number says so."""
    ordered, n = sorted(differences), len(differences)
    if not n:
        raise Refused("no paired rows to compare")
    rank, tail = 0, 0.0
    for below in range(n):
        more = tail + math.comb(n, below) / 2 ** n
        if more > (1 - confidence) / 2:
            break
        rank, tail = below + 1, more
    low, high = (ordered[rank - 1], ordered[n - rank]) if rank else (ordered[0], ordered[-1])
    return {"median": statistics.median(ordered), "low": low, "high": high, "pairs": n,
            "confidence": round(1 - 2 * tail if rank else 1 - 2 * 0.5 ** n, 4)}


def per_task(rows):
    """One number per task and metric for a single run: the sums a pair is the difference of."""
    out = {}
    for row in rows:
        mine = out.setdefault(row["task"], dict.fromkeys(EFFICIENCY + ("reward",), 0))
        mine["tokens"] += sum((row.get(k) or 0) for k in ("input", "cacheRead", "output"))
        for key in ("costUsd", "wallSec", "reward"):
            mine[key] += row.get(key) or 0
    return out


def judge(baseline, runs, floors):
    """A candidate's verdict against the baseline it was paired with, run for run. It is
    `better` only when both gates pass and a whole efficiency interval, at the confidence
    asked for, lies below zero; a median alone is never a win."""
    pairs = [(per_task(base), per_task(mine)) for base, mine in zip(baseline, runs)]
    intervals = {key: interval([mine[task][key] - base[task][key]
                                for base, mine in pairs for task in base if task in mine])
                 for key in EFFICIENCY + ("reward",)}
    flat = [row for run in runs for row in run]
    reason = gate(measure([row for run in baseline for row in run], floors), measure(flat, floors), floors)
    won = any(intervals[k]["high"] < 0 and intervals[k]["confidence"] >= CONFIDENCE for k in EFFICIENCY)
    verdict = "better" if won and not reason else "inconclusive"
    if reason and reason != "no_efficiency_gain":
        verdict = "rejected"
    return {"verdict": verdict, "reason": reason, "intervals": intervals, "measured": measure(flat, floors)}


def grid(knobs, tasks, run, floors, listed, k=3, max_runs=MAX_RUNS):
    """Every point of a small grid against one baseline, paired by repetition and interleaved
    so drift in the provider lands on both sides. Bounded before it starts: at most
    `MAX_KNOBS` knobs, and `(points + 1) * k` runs may not pass `max_runs`. No model is fitted
    over the points (section 10.4): each is judged on its own pairs, then the nondominated
    among the ones that passed survive. `run(overrides, tasks)` is the only thing that costs."""
    if not 1 <= len(knobs) <= MAX_KNOBS:
        raise Refused(f"a grid takes 1 to {MAX_KNOBS} knobs, not {len(knobs)}")
    points = [candidate(dict(zip(knobs, values)), listed) for values in itertools.product(*knobs.values())]
    points = [point for point in points if any(listed[name]["default"] != value for name, value in point.items())]
    planned = (len(points) + 1) * k
    if not points or k < 1:
        raise Refused("nothing to run: every point is the defaults, or k is below 1")
    if planned > max_runs:
        raise Refused(f"{len(points)} points at k={k} is {planned} runs; the bound is {max_runs}")
    stray = sorted(set(tasks) - set(classes(floors)))
    if stray:
        raise Refused(f"task {stray[0]!r} has no class in floors.json")
    baseline, runs = [], [[] for _ in points]
    for repetition in range(k):
        order = [None, *range(len(points))]
        for index in order if repetition % 2 == 0 else reversed(order):
            rows = run({} if index is None else points[index], tasks)
            (baseline if index is None else runs[index]).append(rows)
    judged = [dict(judge(baseline, mine, floors), levers=point) for point, mine in zip(points, runs)]
    passed = {json.dumps(row["levers"], sort_keys=True): row["measured"] for row in judged if not row["reason"]}
    return {"spend": {"runs": planned, "trials": sum(len(rows) for rows in baseline + sum(runs, []))},
            "points": judged, "survivors": survivors(passed)}


def compare(lever, value, tasks, run, floors, listed, k=3):
    """One knob, one value, paired with the baseline: a grid of one point."""
    return grid({lever: [value]}, tasks, run, floors, listed, k=k, max_runs=2 * k)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selfcheck", action="store_true", help="hold the manifest, the fixture and the floors together")
    verbs = parser.add_subparsers(dest="verb")
    for verb in ("compare", "grid"):
        sub = verbs.add_parser(verb)
        sub.add_argument("--runner", required=True, help="the owner's scoring command; this file starts no model")
        sub.add_argument("--group", choices=("development", "validation"), default="development",
                         help="validation is selection: say so in the ledger row")
        sub.add_argument("--k", type=int, default=3)
        if verb == "compare":
            sub.add_argument("--lever", required=True)
            sub.add_argument("--value", type=int, required=True)
        else:
            sub.add_argument("--knob", action="append", required=True, help="name=v1,v2")
            sub.add_argument("--max-runs", type=int, default=MAX_RUNS)
    args = parser.parse_args(argv)
    if args.verb:
        return search(args)
    if not args.selfcheck:
        parser.error("nothing to do; pass --selfcheck, compare or grid")
    found = selfcheck()
    for line in found:
        print(line, file=sys.stderr)
    print(f"levers: {len(manifest())} listed, {sum(1 for r in manifest().values() if r['tunable'])} tunable")
    return 1 if found else 0


def search(args):
    def run(overrides, tasks):
        with tempfile.NamedTemporaryFile("w", suffix=".json") as sink:
            json.dump(overrides, sink)
            sink.flush()
            done = subprocess.run([*shlex.split(args.runner), sink.name, *tasks],
                                  capture_output=True, text=True, check=True)
        return [json.loads(line) for line in done.stdout.splitlines() if line.strip()]

    tasks, floors, listed = read("split.json")[args.group], read("floors.json"), manifest()
    try:
        if args.verb == "compare":
            result = compare(args.lever, args.value, tasks, run, floors, listed, k=args.k)
        else:
            knobs = {name: [int(v) for v in values.split(",")]
                     for name, values in (knob.split("=", 1) for knob in args.knob)}
            result = grid(knobs, tasks, run, floors, listed, k=args.k, max_runs=args.max_runs)
    except (Refused, ValueError) as refusal:
        print(f"refused: {refusal}", file=sys.stderr)
        return 2
    print(json.dumps(dict(result, group=args.group), sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
