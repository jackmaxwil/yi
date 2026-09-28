"""Seeded task generators for the inner loop (design section 19). Each family module exposes
`make(seed) -> {"prompt", "files", "timeoutSec"}`, `check(seed, workspace) -> (passed, total)` and
`solve(seed, workspace)`. The checker and the solution live here, never in the workspace, so a
rollout cannot read its grader; the same seed on both arms of a comparison is the pairing."""
from . import bugfix, logs, reconcile

FAMILIES = {"bugfix": bugfix, "logs": logs, "reconcile": reconcile}
# Inner A/A, 2026-09-28: level 1 scored full on 12 of its first 13 trials, so each family has
# harder levels whose additions are what level 1 left out (tests/test_inner.py names them).
LEVELS = (1, 2, 3)


def parse(task_id):
    """`family:seed[:level]` -> (family, seed, level); an unknown family or level is refused."""
    parts = task_id.split(":")
    if len(parts) == 2:
        parts.append("1")
    if len(parts) != 3 or parts[0] not in FAMILIES or not parts[1].isdigit() or parts[2] not in {str(l) for l in LEVELS}:
        raise ValueError(f"not an inner task id: {task_id!r}")
    return parts[0], int(parts[1]), int(parts[2])
