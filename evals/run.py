#!/usr/bin/env python3
"""Task-eval runner: one `yi ask --json` rollout per fixture task, scored by
the task's own reward.sh in a throwaway copy of its repo.

    python3 evals/run.py --dry --binary target/debug/yi --model faux/faux-1

`--dry` is the offline gate (`just postmerge-evals`): faux only, and every
task's reward must equal the `dryReward` its task.json declares. Faux echoes the
prompt and never calls a tool, so the dry tier proves runner mechanics -- answer
file written, reward invoked in the workspace copy -- not task solving.

Without `--dry` it prints one JSON row per task and a ready-to-paste
docs/eval-ledger.md row. Appending that row stays a human act: a real-model run
is budgeted and user-run (README, "Budget discipline"; plan law 3).
"""

import argparse
import atexit
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT))

import atif  # noqa: E402
import yi_usage  # noqa: E402

TASKS = ROOT / "fixtures" / "tasks"
LIVE = TASKS.parent / "live"
REWARD_TIMEOUT_SEC = 120


def _capture(command):
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    return done.stdout.strip() or "unknown"


def final_answer(events_path):
    """The last assistant text of the rollout.

    SWE-Atlas QnA is graded only from the answer file (README, "Running a
    suite"), so a runner that never writes it scores every real QnA rollout 0.
    """
    text = ""
    events, _ = yi_usage.json_lines(events_path)
    for event in events:
        if event.get("type") != "message_end":
            continue
        message = event.get("message")
        if not isinstance(message, dict) or message.get("role") != "assistant":
            continue
        content = message.get("content")
        if isinstance(content, str):
            blocks = [content]
        elif isinstance(content, list):
            blocks = [
                block.get("text", "")
                for block in content
                if isinstance(block, dict) and block.get("type") == "text"
            ]
        else:
            blocks = []
        joined = "".join(blocks)
        if joined:
            text = joined
    return text


def score(task_dir, workspace, keep=None):
    """Binary reward: reward.sh exits 0, or the task scored nothing.

    cwd is the workspace copy, never the fixture: a reward that reads the
    fixture tree scores the seed instead of the rollout. A task in harbor's
    layout runs its own tests/test.sh with APP, TESTS and LOGS pointed at the
    copy, the verifier image and a scratch dir, so one script serves both
    runners (plan S3).
    """
    verifier = task_dir / "tests" / "test.sh"
    if verifier.is_file():
        command = ["bash", str(verifier)]
        env = {**os.environ, "APP": str(workspace), "TESTS": str(verifier.parent),
               "LOGS": str(Path(workspace).parent / "verifier")}
    else:
        command, env = ["sh", str(task_dir / "reward.sh")], None
    # Incident: a seen-red rollout scored 0 with a final answer that quoted red
    # then green, and nothing kept the verifier's own words; --out keeps them.
    log = (Path(keep) / "verifier.log").open("w") if keep else subprocess.DEVNULL
    try:
        done = subprocess.run(
            command,
            cwd=str(workspace),
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            timeout=REWARD_TIMEOUT_SEC,
        )
    except subprocess.TimeoutExpired:
        return 0
    return 1 if done.returncode == 0 else 0


def run_task(task_dir, binary, model, out=None):
    """One rollout: copy the repo, ask, write the answer file, score."""
    spec = json.loads((task_dir / "task.json").read_text())
    harbor_layout = (task_dir / "instruction.md").is_file()
    prompt = (task_dir / ("instruction.md" if harbor_layout else "prompt.txt")).read_text().strip()
    seed = task_dir / ("environment/app" if harbor_layout else "repo")
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="yi-eval-") as directory:
        # Invariant: the graded tree holds the task's own files plus answer.txt
        # when the rollout answered. The stream and the session dir are siblings,
        # never inside it: a diff or clean-tree reward scores them, and the agent
        # can read its own events.
        workspace = Path(directory) / "repo"
        shutil.copytree(seed, workspace)
        keep = Path(out) / spec["id"] if out else Path(directory)
        keep.mkdir(parents=True, exist_ok=True)
        sessions = keep / "sessions"
        events = keep / "events.jsonl"
        command = [
            binary,
            "ask",
            "--model",
            model,
            "--json",
            "--here",
            "--yolo",
            "--cwd",
            str(workspace),
            "--session-dir",
            str(sessions),
            "--deadline",
            str(spec.get("timeoutSec", 600)),
            # The binary reads YI_LEVERS only under this flag, and only a harness passes
            # it (D220); an older binary that has no flag never sees it either.
            *(["--eval"] if os.environ.get(yi_usage.LEVERS_ENV) else []),
            prompt,
        ]
        timed_out = False
        exit_code = None
        with events.open("w") as sink:
            try:
                # A timeout is never retried: a failed run is a result, and the
                # partial stream is already on disk (README, budget discipline).
                done = subprocess.run(
                    command,
                    stdout=sink,
                    stderr=subprocess.DEVNULL,
                    stdin=subprocess.DEVNULL,
                    env={**os.environ, "HOME": run_home()},
                    timeout=spec.get("timeoutSec", 600),
                )
                exit_code = done.returncode
            except subprocess.TimeoutExpired:
                timed_out = True
        answer = final_answer(events)
        # Incident: row 0012's binary exited 4 before any request, and the empty
        # answer.txt written for it passed clean-workspace; no answer, no file.
        if answer:
            (workspace / "answer.txt").write_text(answer)
        row = {
            "task": spec["id"],
            "reward": score(task_dir, workspace, keep if out else None),
            "exit": exit_code,
            "timedOut": timed_out,
            "wallSec": round(time.monotonic() - started, 2),
        }
        row.update(yi_usage.parse_events(events))
        row.update(yi_usage.session_extras(sessions))
        if out:
            # The per-task record evals/axes.py reads beside the sessions (plan S2):
            # a campaign could not tell which task failed without re-running the suite.
            (keep / "row.json").write_text(json.dumps(row, sort_keys=True))
            write_trajectory(sessions, keep / "trajectory.json", model)
    return spec, row


def write_trajectory(sessions, target, model):
    """The session file as ATIF-v1.7 beside the events (plan S4, E10)."""
    lines = []
    for path in sorted(Path(sessions).rglob("*.jsonl")):
        if not path.name.endswith(".telemetry.jsonl"):
            lines.extend(path.read_text(errors="replace").splitlines())
    if lines:
        target.write_text(json.dumps(atif.convert(lines, model=model), indent=2) + "\n")


def _total(rows, key):
    return sum(row.get(key) or 0 for row in rows)


def ledger_row(rows, model, fingerprint, suite, note="evals/run.py"):
    """The docs/eval-ledger.md row a real run is pasted from, run-id blank."""
    cost = _total(rows, "costUsd")
    unknown = _total(rows, "costUnknownTurns")
    return " | ".join(
        [
            "| NNNN",
            datetime.now(timezone.utc).date().isoformat(),
            suite,
            model,
            fingerprint,
            f"{_total(rows, 'reward')}/{len(rows)}",
            "k=1",
            f"{_total(rows, 'input')}/{_total(rows, 'cacheRead')}/{_total(rows, 'output')}",
            # `?` and `-` are different claims: unmeasurable, versus measured
            # at zero. A run with a usage-less turn cannot report a total.
            "?" if unknown else (f"{cost:.4f}" if cost else "-"),
            f"{_total(rows, 'wallSec'):.1f}s",
            str(_total(rows, "n_agent_steps")),
            str(max((row.get("peak_context_tokens") or 0) for row in rows)),
            str(_total(rows, "summarization_count")),
            f"{note} |",
        ]
    )


_RUN_HOME = []


def run_home():
    """A fresh absolute HOME per process: a run is a harness, never the caller's ~/.yi."""
    if not _RUN_HOME:
        _RUN_HOME.append(tempfile.mkdtemp(prefix="yi-evals-home-"))
        # Incident: each run left its HOME behind, ~650 MB of kernel venv and uv cache;
        # 95 of them filled the disk.
        atexit.register(shutil.rmtree, _RUN_HOME[0], True)
        config = Path(_RUN_HOME[0]) / ".yi" / "config.json"
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text(json.dumps(yi_usage.eval_config(os.environ)))
    return _RUN_HOME[0]


INCONCLUSIVE_MARKS = ("HTTP 5", "timed out", "rate limit", "overloaded", "connection")


def status_of(row, spec, events_text=""):
    """pass, fail or inconclusive: only the last is the weather, never the code."""
    if row.get("timedOut"):
        return "inconclusive", "timeout"
    if row.get("exit") == 4:
        return "inconclusive", "no key"
    if row["reward"] == 1:
        return "pass", ""
    lowered = events_text.lower()
    for mark in INCONCLUSIVE_MARKS:
        if mark.lower() in lowered:
            return "inconclusive", f"provider: {mark}"
    if row.get("costUsd") is None and row.get("costUnknownTurns"):
        return "inconclusive", "provider reported no usage"
    return "fail", f"reward {row['reward']}, want 1"


def refusal_check(spec, binary, out):
    """A refusal scenario asks the binary something it must refuse, and reads the reason."""
    command = [binary, *spec["args"]]
    done = subprocess.run(command, capture_output=True, text=True, timeout=60,
                          env={**os.environ, "HOME": run_home()})
    want_exit = spec.get("expectExit", 2)
    want_text = spec.get("expectStderr", "")
    ok = done.returncode == want_exit and want_text in done.stderr
    status = "pass" if ok else "fail"
    detail = "" if ok else f"exit {done.returncode}, stderr {done.stderr.strip()[:120]!r}"
    return {"task": spec["id"], "reward": 1 if ok else 0, "exit": done.returncode,
            "status": status, "detail": detail, "wallSec": 0}


def cache_check(spec, binary, model, out):
    """A cache scenario asks one session several times; the warm turns must read the cache."""
    keep = out / spec["id"]
    keep.mkdir(parents=True, exist_ok=True)
    sessions = keep / "sessions"
    started = time.monotonic()
    turns, events_text, exit_code, timed_out = [], "", None, False
    with tempfile.TemporaryDirectory(prefix="yi-eval-") as workspace:
        for index, prompt in enumerate(spec["turns"]):
            events = keep / f"events-{index}.jsonl"
            command = [binary, "ask", "--model", model, "--json", "--here", "--yolo", "--cwd", workspace,
                       "--session-dir", str(sessions), *(["--continue"] if index else []), prompt]
            with events.open("w") as sink:
                try:
                    done = subprocess.run(command, stdout=sink, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL,
                                          env={**os.environ, "HOME": run_home()}, timeout=spec.get("timeoutSec", 120))
                    exit_code = done.returncode
                except subprocess.TimeoutExpired:
                    timed_out = True
                    break
            events_text += events.read_text(errors="replace")
            turns.append(yi_usage.parse_events(events))
            # DeepSeek builds its prefix cache after the response, not during it.
            if index + 1 < len(spec["turns"]):
                time.sleep(spec.get("settleSec", 3))
    warm_read = sum(turn["cacheRead"] or 0 for turn in turns[1:])
    row = {"task": spec["id"], "reward": 1 if warm_read else 0, "exit": exit_code, "timedOut": timed_out,
           "wallSec": round(time.monotonic() - started, 2), "requests": len(turns)}
    for key in (*yi_usage.TOKEN_KEYS, "nAssistantMessages", "costUnknownTurns"):
        row[key] = sum(turn.get(key) or 0 for turn in turns)
    costs = [turn.get("costUsd") for turn in turns]
    row["costUsd"] = None if not turns or None in costs else sum(costs)
    row["status"], row["detail"] = status_of(row, spec, events_text)
    if row["status"] == "fail":
        row["detail"] = f"warm turns read 0 cached tokens over {len(turns)} requests ({row['input']} input)"
    return row


def doctor_after(binary):
    """`yi doctor --json` under the run's HOME after every scenario: a red row is a class."""
    done = subprocess.run([binary, "doctor", "--json"], capture_output=True, text=True,
                          timeout=60, env={**os.environ, "HOME": run_home()})
    try:
        rows = json.loads(done.stdout or "[]")
    except json.JSONDecodeError:
        return ["doctor: unreadable output"]
    return [f"Invariant::{row['name']}" for row in rows if row.get("status") == "fail"]


def run_live(args):
    """The live lane: real model, capped in code, deterministic verdicts (D133)."""
    out = Path(args.out) if args.out else Path(tempfile.mkdtemp(prefix="yi-live-"))
    out.mkdir(parents=True, exist_ok=True)
    home = Path(run_home())
    (home / ".yi").mkdir(parents=True, exist_ok=True)
    (home / ".yi" / "config.json").write_text(json.dumps(yi_usage.eval_config(os.environ)))
    specs = sorted(path.parent for path in LIVE.glob("*/task.json"))
    if args.task:
        specs = [spec for spec in specs if spec.name in set(args.task)]
    suite = f"live@{_capture(['git', '-C', str(ROOT), 'rev-parse', '--short', 'HEAD'])}"
    mode = "live" + yi_usage.routing_label(os.environ) + yi_usage.levers_label(os.environ)
    fingerprint = yi_usage.config_fingerprint(_capture([args.binary, "--version"]), args.model, mode, suite)
    rows, spent, budget_hit = [], 0.0, False
    for task_dir in specs:
        spec = json.loads((task_dir / "task.json").read_text())
        if budget_hit:
            rows.append({"task": spec["id"], "status": "inconclusive", "detail": "budget", "reward": 0})
            continue
        if spec.get("kind") == "refusal":
            row = refusal_check(spec, args.binary, out)
        elif spec.get("kind") == "cache":
            row = cache_check(spec, args.binary, args.model, out)
            spent += row.get("costUsd") or 0.0
        else:
            spec, row = run_task(task_dir, args.binary, args.model, out=out)
            events_text = (out / spec["id"] / "events.jsonl").read_text(errors="replace")
            row["status"], row["detail"] = status_of(row, spec, events_text)
            spent += row.get("costUsd") or 0.0
        classes = doctor_after(args.binary)
        if classes and row["status"] == "pass":
            row["status"], row["detail"] = "fail", "; ".join(classes)
        row["classes"] = classes
        row["configFp"] = fingerprint
        rows.append(row)
        if spent > args.cap_usd:
            budget_hit = True
    rollup = {}
    done = subprocess.run([args.binary, "stats", "--json", "telemetry", str(out)],
                          capture_output=True, text=True, env={**os.environ, "HOME": str(home)})
    if done.returncode == 0:
        try:
            rollup = json.loads(done.stdout)
        except json.JSONDecodeError:
            rollup = {}
    counts = {status: sum(1 for row in rows if row["status"] == status) for status in ("pass", "fail", "inconclusive")}
    record = {"suite": suite, "model": args.model, "configFp": fingerprint, "capUsd": args.cap_usd,
              "spentUsd": round(spent, 6), "budgetHit": budget_hit, "counts": counts, "rows": rows, "telemetry": rollup}
    (out / "run.json").write_text(json.dumps(record, indent=1, sort_keys=True))
    print(json.dumps({"out": str(out), "counts": counts, "spentUsd": record["spentUsd"]}, sort_keys=True))
    return 1 if counts["fail"] else 0


def main(argv=None):
    parser = argparse.ArgumentParser(description="run the yi task-eval fixtures")
    parser.add_argument("--binary", default="target/debug/yi")
    parser.add_argument("--home", help="HOME for every run; default a fresh temporary "
                                       "directory, never the caller's")
    parser.add_argument("--model", default="faux/faux-1")
    parser.add_argument("--dry", action="store_true")
    parser.add_argument("--task", action="append", metavar="ID")
    # A lever that leaves --binary and --model alone still has to read back as a
    # different run, so the label rides the fingerprint's mode field.
    parser.add_argument("--variant", default="")
    parser.add_argument("--live", action="store_true",
                        help="the live lane: a real model against evals/fixtures/live, capped")
    parser.add_argument("--cap-usd", type=float, default=1.0)
    parser.add_argument("--out", help="keep sessions, events, row.json and run.json here")
    parser.add_argument("--allow-faux", action="store_true",
                        help="let --live run faux: proves the lane's plumbing offline, never a model")
    args = parser.parse_args(argv)
    # The uv cache is the caller's, so a run HOME's venv costs a clone. Set before any run: a call
    # site builds `{**os.environ, "HOME": run_home()}`, which copies the environment first.
    os.environ.setdefault("UV_CACHE_DIR", str(Path.home() / ".cache" / "uv"))
    if args.home:
        if not os.path.isabs(args.home):
            errors_early = f"--home must be absolute, not {args.home!r}"
            print(errors_early, file=sys.stderr)
            return 2
        # The fixtures lane leaves a caller's HOME as it found it (the live lane writes its config),
        # so the routing the fingerprint's mode would name never reaches that config.
        if os.environ.get(yi_usage.ROUTING_ENV) and not args.live:
            print(f"{yi_usage.ROUTING_ENV} needs the run's own HOME: --home keeps its own config", file=sys.stderr)
            return 2
        _RUN_HOME.append(args.home)

    if args.live:
        if args.model.startswith("faux/") and not args.allow_faux:
            print("--live needs a real model: faux proves nothing here (--allow-faux for the plumbing)", file=sys.stderr)
            return 2
        if not Path(args.binary).is_file():
            print(f"no binary at {args.binary} (cargo build -p yi-cli)", file=sys.stderr)
            return 2
        return run_live(args)
    errors = []
    if args.dry and not args.model.startswith("faux/"):
        errors.append(f"--dry is faux-only, not {args.model}: a gate spends no API budget")
    if not Path(args.binary).is_file():
        errors.append(f"no binary at {args.binary} (cargo build -p yi-cli)")
    tasks = sorted(path.parent for path in TASKS.glob("*/task.json"))
    if args.task:
        wanted = set(args.task)
        missing = wanted - {task.name for task in tasks}
        if missing:
            errors.append(f"no such task: {', '.join(sorted(missing))}")
        tasks = [task for task in tasks if task.name in wanted]
    if not tasks and not errors:
        errors.append(f"no tasks under {TASKS}")
    if errors:
        return report(errors, 0)

    suite = f"fixtures@{_capture(['git', '-C', str(ROOT), 'rev-parse', '--short', 'HEAD'])}"
    mode = (f"yolo+{args.variant}" if args.variant else "yolo") + yi_usage.routing_label(os.environ) + yi_usage.levers_label(os.environ)
    fingerprint = yi_usage.config_fingerprint(
        _capture([args.binary, "--version"]), args.model, mode, suite
    )
    rows = []
    for task in tasks:
        spec, row = run_task(task, args.binary, args.model, out=args.out)
        row["configFp"] = fingerprint
        rows.append(row)
        want = spec.get("dryReward")
        if args.dry and row["reward"] != want:
            errors.append(f"{row['task']}: reward {row['reward']}, want {want}")
    if args.dry:
        return report(errors, len(rows))
    for row in rows:
        print(json.dumps(row, sort_keys=True))
    note = f"evals/run.py {mode}" + (f" tasks={','.join(sorted(args.task))}" if args.task else "")
    print(ledger_row(rows, args.model, fingerprint, suite, note))
    return 0


def report(errors, count):
    """One line per task, matching _common.fail's shape: the exit code is the
    gate, and check_guardrails.sh reads nothing else."""
    if errors:
        print("FAIL evals_run")
        for error in errors:
            print(f"  {error}")
        return 1
    print(f"ok   evals_run ({count} tasks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
