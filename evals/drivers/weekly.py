#!/usr/bin/env python3
"""The weekly evals: today's main against last Monday's, paired, under one hard total.

    EVAL_RUN_ID=<id> python3 evals/drivers/weekly.py --base <yi> --head <yi> --out DIR [--trend] [--dry]

Each leg runs the inner runner (evals/inner/runner.py, the runner protocol) once per arm and
repetition, base and head interleaved on the same seeds, and judges the pair with `levers.judge`.
The capability leg is revert level 3, whose pass rate sits mid-range (rows 0080-0081: 9 of 16), and
the economy leg the generated families, which saturate and so measure cost and wall (section 19 of
docs/plans/2026-09-26-self-improvement-evals.md). Terminal-Bench is no weekly leg: it checks that a
refine winner transfers, and `--trend` (the first week of a month) runs its slice once on head.
Before each call the leg's predicted spend is checked against what its cap and the total have left;
a leg that would pass either stops and is reported as capped, never run past its cap. A revert
seed whose repo history holds no level-3 commit is an errored row on both arms, which the judge drops.
"""
import argparse, json, os, pathlib, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import levers  # noqa: E402

RUNNER = ROOT / "inner/runner.py"
HARBOR = ROOT / "drivers/tbv4_sweep.sh"
import suites  # noqa: E402

REGISTRY = suites.load()


def leg(name, suite, k, cap):
    """A leg's tasks and per-trial price from the registry; the suite must be calibrated."""
    row = suites.gateable(REGISTRY, suite)
    tasks = [task for group in ("development", "validation") for task in row["groups"].get(group, [])]
    return {"name": name, "suite": suite, "tasks": tasks, "k": k, "trialUsd": row["trialUsd"], "cap": cap}


LEGS = (leg("capability", "inner-capability", 1, 1.70), leg("economy", "inner-economy", 1, 1.10))
TREND = {"name": "tbv4-trend", "trials": 12, "trialUsd": REGISTRY["suites"]["tbv4"]["trialUsd"], "cap": 4.00}
# live-weekly's adapter smokes run in their own workflow under these caps ($0.50 + $0.50).
SMOKES_USD = 1.00
TOTAL_USD = 9.50


def hard_caps():
    return sum(leg["cap"] for leg in LEGS) + TREND["cap"] + SMOKES_USD


def spent(rows):
    return sum(levers.UNPRICED_USD if levers.unpriced(row) else levers.value(row, "costUsd") for row in rows)


def call(binary, arm, tasks, run_id):
    with tempfile.NamedTemporaryFile("w", suffix=".json") as overrides:
        overrides.write("{}")
        overrides.flush()
        env = {**os.environ, "EVAL_BINARY": str(binary), "EVAL_ARM": arm, "EVAL_RUN_ID": run_id}
        done = subprocess.run([sys.executable, str(RUNNER), overrides.name, *tasks], env=env,
                              capture_output=True, text=True, check=False)
    rows = [json.loads(line) for line in done.stdout.splitlines() if line.strip().startswith("{")]
    return [dict(row, arm=arm) for row in rows]


def run_leg(leg, args, budget_left):
    tasks = leg["tasks"]
    floors = {leg["name"]: {"pass_min": 0.0, "reward_min": 0.0, "tolerance": 0.0, "tasks": sorted(tasks)}}
    runs, rows, stopped = {"base": [], "head": []}, [], None
    for _ in range(leg["k"]):
        for arm, binary in (("base", args.base), ("head", args.head)):
            predicted = len(tasks) * leg["trialUsd"]
            left = min(leg["cap"], budget_left) - spent(rows)
            if predicted > left:
                stopped = f"capped: ${predicted:.2f} predicted, ${left:.2f} left of the ${leg['cap']:.2f} leg cap"
                break
            got = [] if args.dry else call(binary, arm, tasks, args.run_id)
            rows += got
            runs[arm].append(got)
        if stopped:
            break
    pairs = min(len(runs["base"]), len(runs["head"]))
    verdict = None
    if pairs and not args.dry and tasks:
        verdict = levers.judge(runs["base"][:pairs], runs["head"][:pairs], floors)
    return {"leg": leg["name"], "tasks": len(tasks), "repetitions": pairs,
            "spentUsd": round(spent(rows), 6), "stopped": stopped, "verdict": verdict,
            "predictedUsd": round(len(tasks) * leg["trialUsd"] * 2 * leg["k"], 2)}


def run_trend(args, budget_left):
    predicted = TREND["trials"] * TREND["trialUsd"]
    if predicted > min(TREND["cap"], budget_left):
        return {"leg": TREND["name"], "stopped": f"capped: ${predicted:.2f} predicted", "spentUsd": 0.0}
    if args.dry:
        return {"leg": TREND["name"], "predictedUsd": predicted, "spentUsd": 0.0}
    env = {**os.environ, "EVAL_BINARY": str(args.head), "TBV4_RUNS_DIR": str(args.out / "tbv4"),
           "TBV4_LIST": str(ROOT / "drivers/tbv4_slice.txt"), "TBV4_HARD_CAP": str(TREND["cap"])}
    done = subprocess.run(["sh", str(HARBOR)], env=env, capture_output=True, text=True, check=False)
    return {"leg": TREND["name"], "exit": done.returncode, "predictedUsd": predicted,
            "rowsDir": str(args.out / "tbv4")}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", type=pathlib.Path, required=True, help="last Monday's main, built")
    parser.add_argument("--head", type=pathlib.Path, required=True, help="today's main, built")
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--trend", action="store_true", help="also run the monthly tbv4 trend on head")
    parser.add_argument("--dry", action="store_true", help="predict and print; run nothing")
    args = parser.parse_args(argv)
    args.run_id = os.environ.get("EVAL_RUN_ID", "weekly")
    if hard_caps() > TOTAL_USD:
        print(f"refused: the hard caps sum to ${hard_caps():.2f}, past the ${TOTAL_USD:.2f} total", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    left, legs = TOTAL_USD - SMOKES_USD, []
    for leg in LEGS:
        result = run_leg(leg, args, left)
        left -= result["spentUsd"]
        legs.append(result)
    if args.trend:
        result = run_trend(args, left)
        left -= result["spentUsd"]
        legs.append(result)
    summary = {"runId": args.run_id, "legs": legs, "leftUsd": round(left, 6), "totalUsd": TOTAL_USD}
    (args.out / "verdicts.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    rejected = any((leg.get("verdict") or {}).get("verdict") == "rejected" for leg in legs)
    return 3 if rejected else 0


if __name__ == "__main__":
    sys.exit(main())
