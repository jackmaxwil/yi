"""Seeded task generators for the inner loop (design section 19). Each family module exposes
`make(seed) -> {"prompt", "files", "timeoutSec"}`, `check(seed, workspace) -> (passed, total)` and
`solve(seed, workspace)`. The checker and the solution live here, never in the workspace, so a
rollout cannot read its grader; the same seed on both arms of a comparison is the pairing."""
import shutil
from pathlib import Path

from . import bugfix, logs, mutate, reconcile

# The generated families, graded by construction; they saturate on glm-5.3-flash at every level
# (inner A/A and probe, 2026-09-28) and so measure economy. `mutate` injects bugs into real repos
# for the capability signal.
SYNTHETIC = {"bugfix": bugfix, "logs": logs, "reconcile": reconcile}
FAMILIES = {**SYNTHETIC, "mutate": mutate}
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


def materialize(task, root):
    """The task's workspace under `root`: its repo tree (never .git) with its patch applied, then
    its files. The runner and the tests build workspaces the same way."""
    root = Path(root)
    if task.get("tree"):
        shutil.copytree(task["tree"], root, ignore=shutil.ignore_patterns(".git", ".venv", "__pycache__", "*.pyc"))
    for relative in task.get("drop", []):
        (root / relative).unlink(missing_ok=True)
    for relative, body in {**task.get("patch", {}), **task["files"]}.items():
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body)
    if (root / "run_tests.sh").is_file():
        (root / "run_tests.sh").chmod(0o755)
    return root
