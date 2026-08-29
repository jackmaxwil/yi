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
import json
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))

import yi_usage  # noqa: E402

TASKS = ROOT / "fixtures" / "tasks"
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


def score(task_dir, workspace):
    """TB2.1 binary reward: reward.sh exits 0, or the task scored nothing.

    cwd is the workspace copy, never the fixture: a reward that reads the
    fixture tree scores the seed instead of the rollout.
    """
    try:
        done = subprocess.run(
            ["sh", str(task_dir / "reward.sh")],
            cwd=str(workspace),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=REWARD_TIMEOUT_SEC,
        )
    except subprocess.TimeoutExpired:
        return 0
    return 1 if done.returncode == 0 else 0


def run_task(task_dir, binary, model):
    """One rollout: copy the repo, ask, write the answer file, score."""
    spec = json.loads((task_dir / "task.json").read_text())
    prompt = (task_dir / "prompt.txt").read_text().strip()
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="yi-eval-") as directory:
        # Invariant: the graded tree holds the task's own files plus answer.txt.
        # The stream and the session dir are siblings, never inside it: a diff or
        # clean-tree reward scores them, and the agent can read its own events.
        workspace = Path(directory) / "repo"
        shutil.copytree(task_dir / "repo", workspace)
        sessions = Path(directory) / ".yi-sessions"
        events = Path(directory) / "events.jsonl"
        command = [
            binary,
            "ask",
            "--model",
            model,
            "--json",
            "--yolo",
            "--cwd",
            str(workspace),
            "--session-dir",
            str(sessions),
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
                    timeout=spec.get("timeoutSec", 600),
                )
                exit_code = done.returncode
            except subprocess.TimeoutExpired:
                timed_out = True
        (workspace / "answer.txt").write_text(final_answer(events))
        row = {
            "task": spec["id"],
            "reward": score(task_dir, workspace),
            "exit": exit_code,
            "timedOut": timed_out,
            "wallSec": round(time.monotonic() - started, 2),
        }
        row.update(yi_usage.parse_events(events))
        row.update(yi_usage.session_extras(sessions))
    return spec, row


def _total(rows, key):
    return sum(row.get(key) or 0 for row in rows)


def ledger_row(rows, model, fingerprint, suite):
    """The docs/eval-ledger.md row a real run is pasted from, run-id blank."""
    cost = _total(rows, "costUsd")
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
            f"{cost:.4f}" if cost else "-",
            f"{_total(rows, 'wallSec'):.1f}s",
            str(_total(rows, "n_agent_steps")),
            str(max((row.get("peak_context_tokens") or 0) for row in rows)),
            str(_total(rows, "summarization_count")),
            "evals/run.py |",
        ]
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description="run the yi task-eval fixtures")
    parser.add_argument("--binary", default="target/debug/yi")
    parser.add_argument("--model", default="faux/faux-1")
    parser.add_argument("--dry", action="store_true")
    args = parser.parse_args(argv)

    errors = []
    if args.dry and not args.model.startswith("faux/"):
        errors.append(f"--dry is faux-only, not {args.model}: a gate spends no API budget")
    if not Path(args.binary).is_file():
        errors.append(f"no binary at {args.binary} (cargo build -p yi-cli)")
    tasks = sorted(path.parent for path in TASKS.glob("*/task.json"))
    if not tasks and not errors:
        errors.append(f"no tasks under {TASKS}")
    if errors:
        return report(errors, 0)

    suite = f"fixtures@{_capture(['git', '-C', str(ROOT), 'rev-parse', '--short', 'HEAD'])}"
    fingerprint = yi_usage.config_fingerprint(
        _capture([args.binary, "--version"]), args.model, "yolo", suite
    )
    rows = []
    for task in tasks:
        spec, row = run_task(task, args.binary, args.model)
        row["configFp"] = fingerprint
        rows.append(row)
        want = spec.get("dryReward")
        if args.dry and row["reward"] != want:
            errors.append(f"{row['task']}: reward {row['reward']}, want {want}")
    if args.dry:
        return report(errors, len(rows))
    for row in rows:
        print(json.dumps(row, sort_keys=True))
    print(ledger_row(rows, args.model, fingerprint, suite))
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
