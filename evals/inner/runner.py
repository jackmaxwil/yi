#!/usr/bin/env python3
"""The inner loop's runner (docs/plans/2026-09-26-self-improvement-evals.md section 19).

    EVAL_RUN_ID=<id> EVAL_BINARY=<yi> python3 evals/inner/runner.py <overrides.json> <family:seed[:level]>...

The runner protocol `evals/levers.py` calls: one JSON trial row per task on stdout, filed in
evals/trials/<run-id>.jsonl. Each task is generated from its seed into a fresh temp workspace and run
with `yi ask` in auto mode, so Seatbelt contains bash (no network, writes only in the workspace);
yolo gives bash no containment (crates/permission/src/decide.rs, Yolo). The grader lives in the
generator, never in the workspace. `{}` overrides run the defaults; anything else rides in as
YI_LEVERS under --eval. Sessions are kept under ~/Development/yi-runs/<run-id>/<arm>/<task>/ for
mining. EVAL_MODEL picks the model (a solo arm); EVAL_ROLES=1 adds the role pins to the prompt (S0, issue 1134).
INNER_JOBS (default 6) run at once; a call past its hard cap schedules nothing more.
"""
import concurrent.futures, json, os, pathlib, subprocess, sys, tempfile, threading, time

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT / "drivers"))
sys.path.insert(0, str(ROOT / "inner"))

import axes  # noqa: E402
import gen  # noqa: E402
import run as fixtures  # noqa: E402
import trials  # noqa: E402
import yi_usage  # noqa: E402

MODEL = os.environ.get("EVAL_MODEL", "openrouter/z-ai/glm-5.3-flash")
# S0 roles arm (EVAL_ROLES=1): the orchestrator is told which model each duty takes. Config cannot
# pin a model per role (`models` has no reader or worker key), so the pins ride in the prompt.
ROLES = (
    "\n\nWork as an orchestrator and delegate through ipython. Locate and read code with readers: "
    "`await rlm.run(question, role=\"reader\", model=\"openrouter/deepseek/deepseek-v4.1-flash\", "
    "partition=[...])`. Hand a well-specified mechanical edit to a worker: `await rlm.run(brief, "
    "role=\"worker\", model=\"openrouter/z-ai/glm-5.3-flash\")`. Do the hard reasoning, the design of "
    "the change and the final check yourself on your own model.")
# One inner trial, measured by the inner A/A; predicts a call before it starts.
TRIAL_USD = float(os.environ.get("INNER_TRIAL_USD", "0.03"))


def one(task_id, binary, levers, keep, roles=False):
    family, seed, level = gen.parse(task_id)
    module = gen.FAMILIES[family]
    task = module.make(seed, level)
    started = time.monotonic()
    keep.mkdir(parents=True, exist_ok=True)
    earlier = set(axes.scored_sessions(keep / "sessions"))
    with tempfile.TemporaryDirectory(prefix="yi-inner-") as tmp:
        workspace = gen.materialize(task, pathlib.Path(tmp) / "work")
        events = keep / "events.jsonl"
        command = [binary, "ask", "--model", MODEL, "--json", "--here", "--cwd", str(workspace),
                   "--session-dir", str(keep / "sessions"), "--deadline", str(task["timeoutSec"])]
        env = {**os.environ, "HOME": fixtures.run_home()}
        if levers:
            command.append("--eval")
            env[yi_usage.LEVERS_ENV] = levers
        timed_out, code = False, None
        with events.open("w") as sink:
            try:
                code = subprocess.run(command + [task["prompt"] + (ROLES if roles else "")], stdout=sink, stderr=subprocess.DEVNULL,
                                      stdin=subprocess.DEVNULL, env=env, timeout=task["timeoutSec"] + 60).returncode
            except subprocess.TimeoutExpired:
                timed_out = True
        passed, total = module.check(seed, workspace, level)
    row = {"task": task_id, "family": family, "seed": seed, "level": level, "reward": float(passed == total),
           "testsPassed": passed, "testsTotal": total, "traceScored": False, "partialScore": None,
           "verifierUnmeasured": passed is None,
           "timedOut": timed_out, "errored": code not in (0, None) and not timed_out, "exit": code,
           "wallSec": round(time.monotonic() - started, 2), "model": MODEL, "roles": roles}
    row.update(yi_usage.parse_events(events))
    # The root's event stream is not the family's bill: a child's turns live in its own session file,
    # under the root's session dir, and S0's roles arm spends most of its money there.
    # Only this attempt's files: a re-run under the same run-id and arm reuses the keep dir.
    kept = [axes.score(path, keep) for path in axes.scored_sessions(keep / "sessions", earlier)]
    if kept:
        costs = [r["costUsd"] for r in kept]
        row.update({"rootCostUsd": row["costUsd"], "sessions": len(kept), "familyTurns": sum(r["turns"] for r in kept),
                    "costUsd": None if None in costs else round(sum(costs), 6)})
    return row


def main(argv):
    if len(argv) < 2:
        print("usage: runner.py <overrides.json> <family:seed[:level]>...", file=sys.stderr)
        return 1
    overrides, tasks = argv[0], argv[1:]
    run_id, binary = os.environ.get("EVAL_RUN_ID"), os.environ.get("EVAL_BINARY")
    if not run_id or not binary or not os.access(binary, os.X_OK):
        print("refused: EVAL_RUN_ID and an executable EVAL_BINARY are required", file=sys.stderr)
        return 1
    if not os.environ.get("OPENROUTER_API_KEY"):
        print("refused: OPENROUTER_API_KEY is unset", file=sys.stderr)
        return 1
    for task in tasks:
        gen.parse(task)
    levers = "" if pathlib.Path(overrides).read_text().strip().replace(" ", "") == "{}" else str(pathlib.Path(overrides).resolve())
    hard, refused = trials.caps(run_id, len(tasks), per_trial=TRIAL_USD)
    if refused:
        print(f"refused: {refused}", file=sys.stderr)
        return 2
    arm = os.environ.get("EVAL_ARM") or ("defaults" if not levers else "levers")
    root = pathlib.Path.home() / "Development" / "yi-runs" / run_id / arm
    spent, lock, rows = 0.0, threading.Lock(), []
    trials.STORE.mkdir(exist_ok=True)
    pending = list(tasks)

    def work(task):
        nonlocal spent
        with lock:
            if spent >= hard:
                return None
        row = one(task, binary, levers, root / task.replace(":", "-"), os.environ.get("EVAL_ROLES") == "1")
        row.update({"arm": arm, "at": int(time.time())})
        with lock:
            spent += trials.cost(row)
            rows.append(row)
            with (trials.STORE / f"{run_id}.jsonl").open("a") as sink:
                sink.write(json.dumps(row, sort_keys=True) + "\n")
            print(json.dumps(row, sort_keys=True), flush=True)
        return row

    with concurrent.futures.ThreadPoolExecutor(int(os.environ.get("INNER_JOBS", "6"))) as pool:
        list(pool.map(work, pending))
    skipped = len(tasks) - len(rows)
    if skipped:
        print(f"stopped: hard cap ${hard:.2f} reached, {skipped} task(s) not run", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
