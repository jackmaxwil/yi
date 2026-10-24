"""EXP-B3c: does a headless run stop while its own last check still prints FAIL?

One fresh trial per task of the 12-task TB4 slice on main's binary, per family; a task counts
when any stop in its root session was taken over a FAIL line (evals/stops_on_fail.py).
"""
import json, os, sys, tempfile
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))

import stops_on_fail  # noqa: E402

THRESHOLD = ("keep host enforcement of model-declared checks in headless runs only if a stop is taken while "
             "the last ipython cell printed FAIL in at least 2 of 12 tasks on each family that completed all 12 "
             "(--check survives either way); otherwise KILL (no code), reported with its numbers; a family cut "
             "short by quota waits for the next 5-hour window; one that cannot finish before B3-row-check's wave "
             "is UNRESOLVED and B3-row-check is not built; Kimi trials are never moved to OpenRouter")
FAMILIES = ("kimi", "glm-flash")
# Kimi is the subscription's $0; its figure is the OpenRouter K3.1 price (about 40x flash's $1.95).
PREDICTED_USD = {"kimi": 78.0, "glm-flash": 1.95}
STEP = 6
KEEP_AT = 2
SLICE = (EVALS / "drivers" / "tbv4_slice.txt").read_text().split()
ROUTING = {"ignore": ["inference-net"]}


def runs():
    overrides = Path(tempfile.mkdtemp(prefix="x-b3c-")) / "defaults.json"
    overrides.write_text("{}")
    return [{"id": "slice", "harbor": ["env", f"EVAL_ROUTING={json.dumps(ROUTING)}", "sh", "evals/drivers/tbv4_sweep.sh",
                                       "--runner", str(overrides), *SLICE]}]


def runs_root():
    """Where tbv4_sweep.sh --runner keeps a run's trials, by its own rule."""
    return Path(os.environ.get("TBV4_RUNS_ROOT") or Path.home() / "Development" / "yi-runs")


def tasks_on_fail(run):
    """{task: stops taken over a FAIL} for every slice task with a root session in the run."""
    found, trials = {}, runs_root() / run
    # Incident: a sweep refused before harbor starts (no key, docker down) never makes its run dir.
    for row in stops_on_fail.rows([trials] if trials.is_dir() else []):
        if row["task"] in SLICE:
            found[row["task"]] = found.get(row["task"], 0) + row["stopsOnFail"]
    return found


def verdict(rows):
    """KILL as soon as one finished family is under KEEP_AT, since KEEP needs every family."""
    per = {family: {} for family in FAMILIES}
    for row in rows:
        if row.get("run"):
            per.setdefault(row["family"], {}).update(tasks_on_fail(row["run"]))
    hits = {family: sum(1 for n in per[family].values() if n) for family in FAMILIES}
    lines = ", ".join(f"{family} {hits[family]}/{len(per[family])} tasks" for family in FAMILIES)
    done = [family for family in FAMILIES if len(per[family]) >= len(SLICE)]
    if any(hits[family] < KEEP_AT for family in done):
        return "KILL: " + lines
    return ("KEEP: " if len(done) == len(FAMILIES) else "UNRESOLVED: ") + lines
