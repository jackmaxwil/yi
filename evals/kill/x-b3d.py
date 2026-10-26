"""EXP-B3d: do checks written from instruction.md alone tell Harbor's oracle from its planted defects?

Per family, one fresh `yi ask` per task in a clone holding only that task's instruction.md writes
checks.py there; evals/checker_screen.py scores it on the oracle and on one oracle-minus-one-fix
variant per defect, and caches the score beside the run as score.json. `kill.py report` scores a
run the first time it sees it, through Docker: minutes for cumulative-layout-shift's 21 trees.
"""
import json, shutil, statistics, subprocess, sys, tempfile
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))

import axes  # noqa: E402
import checker_screen  # noqa: E402

THRESHOLD = ("kill the checker recipe (no skill, no S5-E4) if on either family the share of checks passing on "
             "Harbor's oracle is under 90% or the median per-task share of planted defects caught is under half; "
             "a catch passes on the oracle and fails on that defect's variant")
FAMILIES = ("kimi", "glm-flash")
# Kimi is the subscription's $0; its figure is the OpenRouter K3.1 price (about 40x flash's $0.05).
PREDICTED_USD = {"kimi": 2.0, "glm-flash": 0.05}
STEP = 7
PRECISION, SENSITIVITY = 0.9, 0.5
SETTLED = ("ok", "timeout")
# The task trees, solution included, sit in the main checkout's ref/; a run that names it saw them.
LEAK = "ref/benchmarks"
PROMPT = ("instruction.md in this directory is a task another agent will be given. That agent works in the "
          "environment the instruction describes, which you cannot see or reach, and you must not do the task. "
          "Write checks.py in this directory: a Python 3 script that judges, from the instruction alone, whether "
          "the task was done right. It runs as `python3 checks.py` inside that environment after the other agent "
          "stops, with no network; use the absolute paths the instruction names, the standard library and what "
          "the instruction says the environment has; it has no browser. Print one line per check, `PASS <id>` or `FAIL <id>`, ids "
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
    """A settled row's score, cached in score.json; a run that kept no checks.py caught nothing.
    Raises Unscorable, OSError or SubprocessError, caching nothing, when the harness failed."""
    base = axes.runs_root("EVAL_KILL_ROOT") / "x-b3d" / row["run"]
    cache, checks, events = base / "score.json", base / "checks.py", base / "events.jsonl"
    if cache.is_file():
        return json.loads(cache.read_text())
    if events.is_file() and LEAK in events.read_text(errors="replace"):
        raise checker_screen.Unscorable(f"its events name {LEAK}, where the oracle lives")
    root = checker_screen.tasks_dir() / row["id"]
    got = (checker_screen.score(row["id"], checker_screen.TASKS[row["id"]], root, checks,
                                checker_screen.docker_runner(root)) if checks.is_file() else
           {"task": row["id"], "checks": 0, "oraclePassed": 0, "sensitivity": 0.0, "caught": {}})
    cache.write_text(json.dumps(got))
    return got


def judge(family, latest):
    """(word, line) for one family over its latest row per task. A task not run, cut, or unscored
    is open: KILL needs a bar failed even with every open task perfect, KEEP needs none open."""
    got, notes = {}, []
    for task in checker_screen.TASKS:
        row = latest.get((family, task))
        why = "not run" if row is None else None if row.get("status") in SETTLED else f"status {row.get('status')}"
        if why is None:
            try:
                got[task] = scored(row)
            except (OSError, subprocess.SubprocessError, checker_screen.Unscorable) as error:
                why = f"unscored, {type(error).__name__}: {error}"
        notes.append(f"{task} " + (why or f"{got[task]['oraclePassed']}/{got[task]['checks']} pass, "
                                          f"sensitivity {got[task]['sensitivity']:.2f}"))
    checks = sum(s["checks"] for s in got.values())
    precision = sum(s["oraclePassed"] for s in got.values()) / checks if checks else 0.0
    median = statistics.median(got[t]["sensitivity"] if t in got else 0.0 for t in checker_screen.TASKS)
    best = statistics.median(got[t]["sensitivity"] if t in got else 1.0 for t in checker_screen.TASKS)
    whole = len(got) == len(checker_screen.TASKS)
    word = ("KILL" if best < SENSITIVITY or whole and precision < PRECISION else
            "KEEP" if whole and median >= SENSITIVITY else "UNRESOLVED")
    return word, (f"{family} precision {precision:.3f} median sensitivity {median:.3f} ({len(got)}/"
                  f"{len(checker_screen.TASKS)} tasks scored: {'; '.join(notes)})")


def verdict(rows):
    """KILL once one family fails a bar its open tasks cannot lift; KEEP once both clear both bars."""
    latest = {(row["family"], row.get("id")): row for row in rows if row.get("run")}
    judged = [judge(family, latest) if any(f == family for f, _ in latest) else ("UNRESOLVED", f"{family} not run")
              for family in FAMILIES]
    words = {word for word, _ in judged}
    word = "KILL" if "KILL" in words else "KEEP" if words == {"KEEP"} else "UNRESOLVED"
    return f"{word}: " + ", ".join(line for _, line in judged)
