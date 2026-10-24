"""EXP-B3d: do checks written from instruction.md alone tell Harbor's oracle from its planted defects?

Per family, one fresh `yi ask` per task in a clone holding only that task's instruction.md writes
checks.py there; evals/checker_screen.py scores it on the oracle and on one oracle-minus-one-fix
variant per defect, and caches the score beside the run as score.json.
"""
import json, shutil, subprocess, sys, tempfile
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))

import checker_screen  # noqa: E402

THRESHOLD = ("kill the checker recipe (no skill, no S5-E4) if on either family the share of checks passing on "
             "Harbor's oracle is under 90% or the median per-task share of planted defects caught is under half; "
             "a catch passes on the oracle and fails on that defect's variant")
FAMILIES = ("kimi", "glm-flash")
# Kimi is the subscription's $0; its figure is the OpenRouter K3.1 price (about 40x flash's $0.05).
PREDICTED_USD = {"kimi": 2.0, "glm-flash": 0.05}
STEP = 7
PRECISION, SENSITIVITY = 0.9, 0.5
PROMPT = ("instruction.md in this directory is a task another agent will be given. That agent works in the "
          "environment the instruction describes, which you cannot see or reach, and you must not do the task. "
          "Write checks.py in this directory: a Python 3 script that judges, from the instruction alone, whether "
          "the task was done right. It runs as `python3 checks.py` inside that environment after the other agent "
          "stops, with no network; use the absolute paths the instruction names, the standard library and what "
          "the instruction says the environment has. Print one line per check, `PASS <id>` or `FAIL <id>`, ids "
          "short and unique; a check that cannot run prints FAIL. Stop once checks.py is written.")


def runs():
    specs = []
    for task in checker_screen.TASKS:
        repo = Path(tempfile.mkdtemp(prefix=f"x-b3d-{task}-"))
        shutil.copy(checker_screen.tasks_dir() / task / "instruction.md", repo / "instruction.md")
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        subprocess.run(["git", "-C", str(repo), "add", "instruction.md"], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "user.name=screen", "-c", "user.email=screen@localhost",
                        "commit", "-q", "-m", "instruction"], check=True)
        specs.append({"id": task, "prompt": PROMPT, "deadline": 900, "repo": str(repo), "keep": ["checks.py"]})
    return specs


def scored(row):
    """The row's score, from its cache or by running its checks; None when no checks.py was kept."""
    import kill
    base = kill.runs_root() / "x-b3d" / row["run"]
    cache, checks = base / "score.json", base / "checks.py"
    if cache.is_file():
        return json.loads(cache.read_text())
    if not checks.is_file():
        return None
    root = checker_screen.tasks_dir() / row["id"]
    spec = checker_screen.TASKS[row["id"]]
    got = checker_screen.score(row["id"], spec, root, checks, checker_screen.docker_runner(root))
    cache.write_text(json.dumps(got))
    return got


def verdict(rows):
    """KILL as soon as one family that ran is under either bar; a task it wrote no checks for caught nothing."""
    per, ran = {family: [] for family in FAMILIES}, {row["family"] for row in rows if row.get("run")}
    for row in rows:
        got = scored(row) if row.get("run") else None
        if got:
            per.setdefault(row["family"], []).append(got)
    figures = {family: checker_screen.family(per[family]) for family in FAMILIES}
    lines = ", ".join(f"{family} precision {p:.3f} median sensitivity {s:.3f} ({len(per[family])}/"
                      f"{len(checker_screen.TASKS)} tasks scored)" for family, (p, s) in figures.items())
    if any(figures[family][0] < PRECISION or figures[family][1] < SENSITIVITY for family in ran & set(FAMILIES)):
        return "KILL: " + lines
    return ("KEEP: " if ran >= set(FAMILIES) else "UNRESOLVED: ") + lines
