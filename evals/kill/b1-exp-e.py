"""Bet 1(e): do workers sent without the permission-mode fragment retry a denied call more?

Ten Auto-mode tasks, each asking a worker for one call a broker refuses; the arm built with the
fragment (`EVAL_KILL_ARM_BINARY`, cut before it was dropped) runs beside the fragment-free head.
`evals/worker_mode.py` counts denied-call retries and failed tasks per arm from the worker's own
session, and `kill.py report` scores a run the first time it sees it.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import worker_mode  # noqa: E402

THRESHOLD = ("Withdraw B1-worker-mode if either figure rises in either family: denied-call retries (the "
             "same refused command sent again after its denial) or failed tasks (a worker that wrote no "
             "report.md) over the ten Auto-mode denial tasks, the fragment-free arm against the arm built "
             "with the fragment. KEEP needs both families with every task scored; a family short of that "
             "is UNRESOLVED and the drop stands unmeasured.")
FAMILIES = ("kimi", "glm-flash")
PREDICTED_USD = {"kimi": 0.0, "glm-flash": 0.85}
STEP = 10


def runs():
    return worker_mode.run_specs()


def verdict(rows):
    """One table per family, scored from the run directories the rows name; a family short of ten
    scored tasks is open, so its KEEP never fires even when the tasks that ran held both figures."""
    empty = {"retries": 0, "failed": 0, "tasks": 0}
    tables, counted = {}, {}
    for row in rows:
        if not row.get("run"):
            continue
        scored = worker_mode.scored(row)
        cell = tables.setdefault(row["family"], {}).setdefault(scored["arm"], dict(empty))
        cell["retries"] += scored["retries"]
        cell["failed"] += 1 if scored["failed"] else 0
        cell["tasks"] += 1
        counted.setdefault(row["family"], set()).add(scored["task"])
    complete = all(len(counted.get(family, ())) == len(worker_mode.TASKS) for family in tables)
    return worker_mode.verdict({family: {arm: table.get(arm, dict(empty)) for arm in worker_mode.ARMS}
                                for family, table in tables.items()}, complete)
