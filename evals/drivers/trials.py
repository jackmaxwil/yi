#!/usr/bin/env python3
"""The trial store and the spend a runner call may still use (design section 5.3).

    python3 evals/drivers/trials.py rows <harbor job dir> --run-id ID [--arm LABEL]
    python3 evals/drivers/trials.py caps --run-id ID --tasks N

`rows` prints one JSON row per harbor trial, however many session files it wrote (a child
session's tokens and cost are the trial's), and appends them to evals/trials/<run-id>.jsonl
with `at` and `arm`. `caps` prints the hard cap in USD this call may spend, or exits 2 naming
the soft cap it would cross. A trial with an unpriced turn counts UNPRICED_USD, the watcher's
per-trial cap it cannot pass: never $0 and never a price table (E14). Stdlib only.
"""
import argparse, json, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "drivers"))

import axes  # noqa: E402
from watch import PER_TRIAL_USD as UNPRICED_USD  # noqa: E402

STORE = ROOT / "trials"
# Owner, 2026-09-27: per stage soft $8 / hard $10; per week soft $25 / hard $30.
STAGE_SOFT, STAGE_HARD, WEEK_SOFT, WEEK_HARD = 8.0, 10.0, 25.0, 30.0
# Row 0055's median priced trial; a call is predicted at this per task before it starts.
TRIAL_USD = 0.13
SUMMED = ("input", "cacheRead", "cacheWrite", "output", "turns")
KEPT = ("task", "trial", "reward", "partialScore", "censored", "errored", "timedOut", "wallSec")


def trial_rows(job):
    """One row per trial directory under a harbor job: its context once, its sessions summed."""
    job = Path(job).resolve()
    merged = {}
    for path in sorted(job.rglob("*.jsonl")):
        if not axes.is_session(path):
            continue
        row = axes.score(path, job)
        mine = merged.get(row["trial"])
        if mine is None:
            merged[row["trial"]] = {**{k: row.get(k) for k in KEPT + SUMMED}, "costUsd": row["costUsd"]}
            continue
        for key in SUMMED:
            mine[key] = (mine[key] or 0) + (row.get(key) or 0)
        mine["costUsd"] = None if mine["costUsd"] is None or row["costUsd"] is None \
            else round(mine["costUsd"] + row["costUsd"], 6)
    return list(merged.values())


def cost(row):
    return UNPRICED_USD if row.get("costUsd") is None else row["costUsd"]


def spend(store, now, run_id=None):
    """(this run's spend, this ISO week's spend) over the store's rows."""
    week = time.gmtime(now)
    this_week = lambda at: time.strftime("%G-%V", time.gmtime(at)) == time.strftime("%G-%V", week)
    run = total = 0.0
    for path in sorted(Path(store).glob("*.jsonl")):
        for line in path.read_text().splitlines():
            row = json.loads(line) if line.strip() else {}
            if "costUsd" not in row:
                continue
            if this_week(row.get("at", 0)):
                total += cost(row)
            if path.stem == run_id:
                run += cost(row)
    return round(run, 6), round(total, 6)


def caps(run_id, tasks, store=STORE, now=None):
    """(hard cap for this call, None) or (0, the soft cap it would cross)."""
    now = time.time() if now is None else now
    run, week = spend(store, now, run_id)
    predicted = tasks * TRIAL_USD
    if run + predicted > STAGE_SOFT:
        return 0.0, f"stage soft cap ${STAGE_SOFT:g}: {run_id} has spent ${run:.2f}, this call is predicted ${predicted:.2f}"
    if week + predicted > WEEK_SOFT:
        return 0.0, f"week soft cap ${WEEK_SOFT:g}: this week has spent ${week:.2f}, this call is predicted ${predicted:.2f}"
    return round(min(STAGE_HARD - run, WEEK_HARD - week), 6), None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    verbs = parser.add_subparsers(dest="verb", required=True)
    rows = verbs.add_parser("rows")
    rows.add_argument("job", type=Path)
    rows.add_argument("--run-id", required=True)
    rows.add_argument("--arm", default="")
    cap = verbs.add_parser("caps")
    cap.add_argument("--run-id", required=True)
    cap.add_argument("--tasks", type=int, required=True)
    args = parser.parse_args(argv)
    if args.verb == "caps":
        hard, refused = caps(args.run_id, args.tasks)
        if refused:
            print(f"refused: {refused}", file=sys.stderr)
            return 2
        print(f"{hard:.2f}")
        return 0
    found = trial_rows(args.job)
    STORE.mkdir(exist_ok=True)
    at = int(time.time())
    with (STORE / f"{args.run_id}.jsonl").open("a") as sink:
        for row in found:
            sink.write(json.dumps({**row, "at": at, "arm": args.arm}, sort_keys=True) + "\n")
    for row in found:
        print(json.dumps(row, sort_keys=True))
    return 0 if found else 2


if __name__ == "__main__":
    sys.exit(main())
