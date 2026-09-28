"""Seeded task generators for the inner loop (design section 19). Each family module exposes
`make(seed) -> {"prompt", "files", "timeoutSec"}`, `check(seed, workspace) -> (passed, total)` and
`solve(seed, workspace)`. The checker and the solution live here, never in the workspace, so a
rollout cannot read its grader; the same seed on both arms of a comparison is the pairing."""
from . import bugfix, logs, reconcile

FAMILIES = {"bugfix": bugfix, "logs": logs, "reconcile": reconcile}


def parse(task_id):
    """`family:seed` -> (family, seed); an unknown family is refused."""
    family, _, seed = task_id.partition(":")
    if family not in FAMILIES or not seed.isdigit():
        raise ValueError(f"not an inner task id: {task_id!r}")
    return family, int(seed)
