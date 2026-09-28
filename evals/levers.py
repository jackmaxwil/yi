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

The runner is the owner's: it is called as `<runner> <overrides.json> <task>...` with
`YI_LEVERS=<overrides.json>` in its environment (this process's own is untouched), and
prints one JSON row per trial. The binary reads that file only in a run that carries `--eval`.

A trial row is the shape `evals/axes.py` scores: `task`, `reward`, `input`, `cacheRead`,
`output`, `costUsd`, `wallSec`, plus `partialScore`, `censored` and `errored` when the runner
has them. The verdict takes one difference per task (repetitions of a task are one cluster):
every class stays within `tolerance` of its floor and no binary pass is lost, then either
graded credit rises with cost and wall within ten percent (capability), or cost, tokens or
wall fall with graded credit within DELTA (economy); among the candidates that pass, only the
nondominated survive. docs/plans/2026-09-26-self-improvement-evals.md section 6.3.
"""
import argparse, itertools, json, math, os, pathlib, re, shlex, statistics, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "evals/graph"))
sys.path.insert(0, str(ROOT / "evals/adapters"))
sys.path.insert(0, str(ROOT / "skills/yi/session-mining"))
from refine import Refused, names  # noqa: E402
from yi_usage import LEVERS_ENV  # noqa: E402

LEVERS = ROOT / "evals/levers"
EFFICIENCY = ("costUsd", "tokens", "wallSec")
MAX_KNOBS, MAX_RUNS, CONFIDENCE = 5, 40, 0.95
# The tasks an interval needs before it can reach CONFIDENCE at all: the whole-sample
# interval covers 1 - 2 / 2**n, so five tasks buy 0.9375 and no more.
MIN_TASKS = math.ceil(math.log2(2 / (1 - CONFIDENCE)))
# Road 1's bound on the cost and wall a capability gain may add: the owner's number (OS plan 10.3).
COST_TOLERANCE = 0.10
# Road 2's non-inferiority margin on graded credit. A placeholder until the slice's A/A run
# measures the task-level spread; it must stay below the smallest gain road 1 would accept.
DELTA = 0.07
# Past this share of (task, repetition) pairs lost on both arms, the run says nothing.
MAX_DROPPED = 0.2
SCATTER = ROOT / "python/yi_runtime/src/yi/shapes.py"


def read(name):
    return json.loads((LEVERS / name).read_text())


def value(row, key):
    """One metric of one trial row. A missing one is zero; a string, a boolean, a NaN or an
    infinity is refused, because a gate cannot compare what does not order: a NaN reward
    compares false against its floor and would pass a gate by not being a number."""
    got = row.get(key) or 0
    if isinstance(got, bool) or not isinstance(got, (int, float)) or not math.isfinite(got):
        raise Refused(f"task {row.get('task')!r} reports {key}={got!r}, which is not a number")
    return got


def manifest():
    return {row["name"]: row for row in read("levers.json")["levers"]}


def selfcheck(load=read):
    """Every problem found, as one line each; an empty list is a pass. `load` is the reader,
    so a test can hand this a doctored manifest and watch each lint fire."""
    rows, default, floors, split = load("levers.json")["levers"], load("default.json"), load("floors.json"), load("split.json")
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
        solved = {row["task"] for row in mine if value(row, "reward") > 0}
        capability[name] = {"pass": len(solved) / len(tasks),
                            "reward": sum(value(row, "reward") for row in mine) / len(mine)}
    tokens = sum(value(row, k) for row in rows for k in ("input", "cacheRead", "output"))
    return {"capability": capability, "tokens": tokens,
            "tasks": sorted({row["task"] for row in rows}),
            "costUsd": sum(value(row, "costUsd") for row in rows),
            "wallSec": sum(value(row, "wallSec") for row in rows)}


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
    # A pass rate is over the tasks that ran, so dropping the hard one raises it: a task the
    # baseline ran and the candidate did not is a missing row, never a saving.
    missing = sorted(set(baseline["tasks"]) - set(candidate["tasks"]))
    if missing:
        return f"task_not_run:{missing[0]}"
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
    """The guard a later fit would have to pass, and nothing more: no command here fits
    anything (section 10.4 forbids it until the data justifies a model). Refused whole when
    any row names a held-out task, by token as `refine.names` reads one."""
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


def graded(row):
    """Partial credit: a trace's own score when `axes.py` found one; else, for a task no trace
    scores, its ctrf test tally (N1: most slice tasks, e.g. production-planning 16/20); else the
    reward. A trace-scored task's ctrf can be a wrapper test that passes beside a failing trace
    (`selftest.py` pins freight-dispatch-shift and vba-userform-port), so it is never read there."""
    if row.get("partialScore") is not None:
        return value(row, "partialScore")
    if not row.get("traceScored") and row.get("testsTotal"):
        return value(row, "testsPassed") / value(row, "testsTotal")
    return value(row, "reward")


def per_task(rows):
    """One number per task and metric for a single run: the sums a pair is the difference of."""
    out = {}
    for row in rows:
        mine = out.setdefault(row["task"], dict.fromkeys(EFFICIENCY + ("reward", "graded"), 0))
        mine["tokens"] += sum(value(row, k) for k in ("input", "cacheRead", "output"))
        for key in ("costUsd", "wallSec", "reward"):
            mine[key] += value(row, key)
        mine["graded"] += graded(row) if usable(row) else 0
    return out


def usable(row):
    """A trial the watcher stopped, that errored, or whose verifier never judged it (N1: verifier
    timeouts under load) scored nothing it can be credited with."""
    return not (row.get("censored") or row.get("errored") or row.get("verifierUnmeasured"))


def unpriced(row):
    return row.get("unmeasured") or ("costUsd" in row and row["costUsd"] is None)


def relative(mine, base):
    return (mine - base) / base if base else (0.0 if mine == base else math.inf)


def task_differences(baseline, runs):
    """One difference per task and metric: the candidate's mean over its repetitions minus the
    base's. Repetitions of one task are one cluster, so pooling them as independent pairs
    overstates the confidence (the 0.282.0 limits). A (task, repetition) unusable on both arms
    is dropped; on one arm only, that arm's trial scores nothing, since a stop or an error is
    part of what the arm did. Returns the differences, and the pairs seen and dropped."""
    cells, seen, dropped = {}, 0, 0
    for base_run, mine_run in zip(baseline, runs):
        base_rows = {row["task"]: row for row in base_run}
        for row in mine_run:
            other = base_rows.get(row["task"])
            if other is None:
                continue
            seen += 1
            if not usable(row) and not usable(other):
                dropped += 1
                continue
            cell = cells.setdefault(row["task"], ([], []))
            cell[0].append(per_task([other])[row["task"]])
            cell[1].append(per_task([row])[row["task"]])
    diffs = {}
    for task, (base, mine) in sorted(cells.items()):
        mean = {side: {key: statistics.fmean(one[key] for one in rows) for key in base[0]}
                for side, rows in (("base", base), ("mine", mine))}
        diffs[task] = {key: mean["mine"][key] - mean["base"][key] for key in base[0]}
        diffs[task]["relCost"] = relative(mean["mine"]["costUsd"], mean["base"]["costUsd"])
        diffs[task]["relWall"] = relative(mean["mine"]["wallSec"], mean["base"]["wallSec"])
    return diffs, seen, dropped


def judge(baseline, runs, floors, accesses=1, delta=DELTA):
    """A candidate's verdict against the fresh base it was paired with, one difference per task.

    Rejected: below a floor, a task or class not run, or fewer binary passes than the base.
    Inconclusive: more than one unpriced trial, too many pairs dropped, or neither road.
    Better by road 1 (capability): the graded interval lies above 0 while the cost and wall
    intervals stay within COST_TOLERANCE of the base. Better by road 2 (economy): an efficiency
    interval lies below 0 while the graded interval stays above -delta. Every interval is at
    confidence 1 - 0.05 / accesses: a validation group looked at `accesses` times spends its
    error rate once per look (Bonferroni)."""
    target = 1 - (1 - CONFIDENCE) / accesses
    diffs, seen, dropped = task_differences(baseline, runs)
    intervals = {key: interval([diff[key] for diff in diffs.values()], target)
                 for key in EFFICIENCY + ("reward", "graded", "relCost", "relWall")}
    base_flat, flat = [row for run in baseline for row in run], [row for run in runs for row in run]
    reason = gate(measure(base_flat, floors), measure(flat, floors), floors)
    if reason == "no_efficiency_gain":
        reason = None

    # Binary passes are judged like everything else, one difference per task: `pass_lost` needs the
    # whole pass interval below zero. Incident: a zero-tolerance count rejected an inner A/A whose
    # second arm fully solved one task fewer of 60.
    lost = intervals["reward"]
    hard = reason or ("pass_lost" if lost["high"] < 0 and lost["confidence"] >= target else None)
    soft = ("unmeasured" if sum(1 for row in base_flat + flat if unpriced(row)) > 1
            else "pairs_dropped" if seen and dropped / seen > MAX_DROPPED else None)
    sure = {key: row["confidence"] >= target for key, row in intervals.items()}
    road = None
    if sure["graded"] and intervals["graded"]["low"] > 0 and all(
            sure[key] and intervals[key]["high"] <= COST_TOLERANCE for key in ("relCost", "relWall")):
        road = "capability"
    elif sure["graded"] and intervals["graded"]["low"] > -delta and any(
            sure[key] and intervals[key]["high"] < 0 for key in EFFICIENCY):
        road = "economy"
    verdict = "rejected" if hard else "better" if road and not soft else "inconclusive"
    return {"verdict": verdict, "reason": hard or soft, "road": None if hard or soft else road,
            "intervals": intervals, "confidence": round(target, 4), "tasks": len(diffs),
            "dropped": dropped, "per_task": {key: {task: diff[key] for task, diff in diffs.items()}
                                             for key in EFFICIENCY + ("reward", "graded")},
            "measured": measure(flat, floors)}


def screen(baseline, runs, floors):
    """The dev stage's rule, which can refuse and never accept: None when the median task's
    graded difference is at least 0, no pass is lost and no floor is broken."""
    verdict = judge(baseline, runs, floors)
    if verdict["verdict"] == "rejected":
        return verdict["reason"]
    graded_diffs = list(verdict["per_task"]["graded"].values())
    if not graded_diffs or statistics.median(graded_diffs) < 0:
        return "median_task_worse"
    return None


def max_cut_rung(entries):
    """The highest `rung` a cut redrive carried: the loop's own cut count (`run.rs`
    `length_redrive`). The cut that ends a run writes no redrive, so a run stopped at the
    default's cut left rung `default - 1` behind."""
    return max((((entry.get("message") or {}).get("details") or {}).get("rung") or 0
                for entry in entries
                if (entry.get("message") or {}).get("customType") == "length_redrive"
                and (((entry.get("message") or {}).get("details") or {}).get("cut"))), default=0)


# The levers whose decision can be replayed from what a session records (design section 6.5).
# loop.length_stop_at is not one: a truncated tool call counts a length stop without a
# redrive. loop.reasoning_cap is not one: a cut aborts at the cap, so no longer reasoning exists.
CENSUS = {"loop.cut_stop_at": max_cut_rung}


def census(lever, candidate, sessions):
    """How many recorded sessions would have stopped differently at `candidate`: the S0 screen
    that refuses, for nothing, a value no session ever met. Below the default a session flips
    when its cuts reached the candidate; above it, when the default's stop ended it."""
    if lever not in CENSUS:
        raise Refused(f"no census for {lever}: its decision is not recorded in a session")
    import extract
    default, flips, seen = manifest()[lever]["default"], 0, 0
    for path in sorted(pathlib.Path(sessions).rglob("*.jsonl")):
        if path.name.endswith(".telemetry.jsonl"):
            continue
        header, entries, _corrupt = extract.read_session(path)
        if not header:
            continue
        seen += 1
        rung = CENSUS[lever](entries)
        flips += rung >= candidate if candidate < default else rung >= default - 1 if candidate > default else 0
    return {"lever": lever, "value": candidate, "default": default, "sessions": seen, "flips": int(flips)}


def grid(knobs, tasks, run, floors, listed, k=3, max_runs=MAX_RUNS, accesses=1):
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
    if len(tasks) < MIN_TASKS:
        print(f"note: {len(tasks)} tasks give {len(tasks)} differences whatever k is, and an "
              f"interval needs {MIN_TASKS} tasks to reach {CONFIDENCE}: no point here can be better",
              file=sys.stderr)
    baseline, runs = [], [[] for _ in points]
    for repetition in range(k):
        order = [None, *range(len(points))]
        for index in order if repetition % 2 == 0 else reversed(order):
            rows = run({} if index is None else points[index], tasks)
            (baseline if index is None else runs[index]).append(rows)
    judged = [dict(judge(baseline, mine, floors, accesses=accesses), levers=point)
              for point, mine in zip(points, runs)]
    passed = {json.dumps(row["levers"], sort_keys=True): row["measured"]
              for row in judged if row["verdict"] == "better"}
    return {"spend": {"runs": planned, "trials": sum(len(rows) for rows in baseline + sum(runs, []))},
            "points": judged, "survivors": survivors(passed)}


def compare(lever, value, tasks, run, floors, listed, k=3, accesses=1):
    """One knob, one value, paired with the baseline: a grid of one point."""
    return grid({lever: [value]}, tasks, run, floors, listed, k=k, max_runs=2 * k, accesses=accesses)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selfcheck", action="store_true", help="hold the manifest, the fixture and the floors together")
    verbs = parser.add_subparsers(dest="verb")
    tally = verbs.add_parser("census", help="sessions a candidate value would have stopped differently")
    tally.add_argument("--lever", required=True)
    tally.add_argument("--value", type=int, required=True)
    tally.add_argument("--sessions", required=True, help="a directory of v4 session files")
    for verb in ("compare", "grid"):
        sub = verbs.add_parser(verb)
        sub.add_argument("--runner", required=True, help="the owner's scoring command; this file starts no model")
        sub.add_argument("--group", choices=("development", "validation"), default="development",
                         help="validation is selection: say so in the ledger row")
        sub.add_argument("--k", type=int, default=3)
        sub.add_argument("--accesses", type=int, default=1,
                         help="validation looks this group has had, this one included (Bonferroni)")
        if verb == "compare":
            sub.add_argument("--lever", required=True)
            sub.add_argument("--value", type=int, required=True)
        else:
            sub.add_argument("--knob", action="append", required=True, help="name=v1,v2")
            sub.add_argument("--max-runs", type=int, default=MAX_RUNS)
    args = parser.parse_args(argv)
    if args.verb == "census":
        try:
            print(json.dumps(census(args.lever, args.value, args.sessions)))
        except Refused as refusal:
            print(f"refused: {refusal}", file=sys.stderr)
            return 2
        return 0
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
            # Outside the repo, gone when the point is done, and named to the child through
            # both its argv and its environment; this process's own is untouched.
            done = subprocess.run([*shlex.split(args.runner), sink.name, *tasks],
                                  env={**os.environ, LEVERS_ENV: sink.name},
                                  capture_output=True, text=True, check=True)
        return [json.loads(line) for line in done.stdout.splitlines() if line.strip()]

    tasks, floors, listed = read("split.json")[args.group], read("floors.json"), manifest()
    try:
        if args.verb == "compare":
            result = compare(args.lever, args.value, tasks, run, floors, listed, k=args.k, accesses=args.accesses)
        else:
            knobs = {name: [int(v) for v in values.split(",")]
                     for name, values in (knob.split("=", 1) for knob in args.knob)}
            result = grid(knobs, tasks, run, floors, listed, k=args.k, max_runs=args.max_runs, accesses=args.accesses)
    except (Refused, ValueError) as refusal:
        print(f"refused: {refusal}", file=sys.stderr)
        return 2
    print(json.dumps(dict(result, group=args.group), sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
