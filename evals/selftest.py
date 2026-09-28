#!/usr/bin/env python3
"""Dry-run gate for the Yi eval adapters: no docker, no API key, no harness.

Each check defends one gate from evals/README.md against a recorded artifact,
so a broken adapter fails here instead of scoring a real rollout 0.

    python3 evals/selftest.py
"""

import contextlib
import io
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT / "arc"))
sys.path.insert(0, str(ROOT / "drivers"))
sys.path.insert(0, str(ROOT / "graph"))
sys.path.insert(0, str(ROOT / "tests"))
sys.path.insert(0, str(ROOT))

import orient_census  # noqa: E402
import rule_fires  # noqa: E402
import record  # noqa: E402
import tb21_cost  # noqa: E402
import cache_probe  # noqa: E402
import atif  # noqa: E402
import run  # noqa: E402
import yi_arc  # noqa: E402
import surface  # noqa: E402
import axes  # noqa: E402
import yi_usage  # noqa: E402
import test_refine  # noqa: E402
import test_levers  # noqa: E402
import test_judge_replay  # noqa: E402
import test_improve  # noqa: E402
import test_skill_labels  # noqa: E402

FIXTURES = ROOT / "fixtures"
EVENTS = FIXTURES / "ask_events.jsonl"
SESSIONS = FIXTURES / "session"
RECORDED = FIXTURES / "session-record"

# The fake credential planted in the faux-echo prompt before it was recorded.
PLANT = "sk-fake-3QpZr7Lm2Xv9Tb4Nc8Kd1Wq6"

# A prompt that would break naive quoting: quotes, a pipe, a variable, newlines,
# and multi-KB length (E7).
INSTRUCTION = (
    "Fix the 'flaky' test | rm -rf $HOME\nsecond line \"quoted\"\n" + "x" * 4096
)


def check_command():
    """E6/E7/E8: the run command harbor and pier both execute."""
    command = yi_usage.run_command("anthropic/claude-opus-4-5", INSTRUCTION)
    argv = shlex.split(command)
    assert "--json" in argv, "E1: the adapter must run the JSON event stream"
    assert "--yolo" in argv, "E8: a permission prompt hangs the trial to timeout"
    # Incident: without it the owner worked in lane 0 and accepted work never reached /app
    # (fan-out and fleet-forensics scored 0, and 1.0 with it).
    assert "--here" in argv, "the grader reads the task's checkout, not a lane"
    # E6: a bare `grep -v` exits 1 when nothing survives and pipefail scores the
    # trial 0; the delta filter is guarded, and tee's stdout is not harbor's to hold.
    assert "grep -v" in command and "|| [ $? -eq 1 ]" in command, "E6: the delta filter must be guarded"
    assert command.rstrip().endswith(">/dev/null"), "harbor must not buffer the event stream"
    assert "message_update" in command, "the deltas are what made one trial 43.8 GB"
    assert f"tee -a {yi_usage.REMOTE_EVENTS_PATH}" in command, "a resumed trial truncated its first segment"
    # Incident: grep block-buffers into a pipe, so a deadline smoke harbor killed left yi.jsonl
    # at 172,032 bytes, cut inside turn 1's turn_end, its cost short and agent_end never written.
    assert "stdbuf -oL grep -v" in command, "the delta filter must pass each event line on as it lands"
    assert argv.count(INSTRUCTION) == 1, "E7: instruction must be one quoted argv"
    assert yi_usage.REMOTE_SESSION_DIR in argv, "session dir must be collected"
    assert yi_usage.REMOTE_SESSION_DIR.startswith("/logs/"), "sessions live under /logs"
    assert "--continue" not in argv, "resume is opt-in"
    assert "--continue" in shlex.split(
        yi_usage.run_command("anthropic/claude-opus-4-5", "hi", resume=True)
    ), "resume must pass --continue"
    # E16: the binary reads YI_LEVERS only under --eval (crates/runtime/src/levers.rs), so a
    # trial with levers carries both, and a trial without them is byte-identical to before.
    assert "--eval" not in argv and yi_usage.LEVERS_ENV not in command, "no levers, no --eval"
    levered = yi_usage.run_command("anthropic/claude-opus-4-5", "hi", levers=True)
    assert "--eval" in shlex.split(levered), "E16: a levers trial must pass --eval"
    assert f"{yi_usage.LEVERS_ENV}={yi_usage.REMOTE_LEVERS_PATH} yi ask " in levered, \
        "E16: the binary must be told where the uploaded levers file is"
    assert yi_usage.REMOTE_LEVERS_PATH.startswith("/logs/agent/"), "levers ride the synced logs"
    assert "--deadline" not in argv, "the deadline is the driver's to name"
    timed = shlex.split(yi_usage.run_command("anthropic/claude-opus-4-5", "hi", deadline_sec=3600))
    assert timed[timed.index("--deadline") + 1] == "3600", "the model must learn its wall clock"
    for bad in ("", "claude-opus-4-5"):
        try:
            yi_usage.run_command(bad, "hi")
        except ValueError:
            continue
        raise AssertionError(f"model name {bad!r} must be rejected")


def check_install():
    """S2: the adapter builds the kernel venv at install and reports an image it cannot boot on;
    #329: it runs the trial anyway, since a Bun image has no python and bash still scores."""
    source = (ROOT / "adapters" / "yi_harbor" / "agent.py").read_text()
    assert "doctor --fix --json" in source and "warm_kernel" in source, "install must warm the kernel"
    assert "cannot boot in this image" not in source, "a dead kernel is reported, not a refusal"
    # E16: the host's YI_LEVERS file reaches the container, the run asks for it, and the
    # fingerprint names it, so a levers row never shares a config column with the defaults.
    assert "REMOTE_LEVERS_PATH" in source and "upload_levers" in source, "E16: levers must be uploaded"
    assert "levers=bool(" in source, "E16: the run must pass the levers flag"
    assert "levers_label(os.environ)" in source, "E16: the harbor fingerprint must name the levers"
    green = json.dumps([{"name": "kernel-toolchain", "status": "ok", "detail": "uv /usr/bin/uv"},
                        {"name": "kernel-boot", "status": "fixed", "detail": "built 20000 ms"}])
    assert yi_usage.kernel_problems(green) == []
    assert yi_usage.kernel_problems("\u203a setting up python kernel (one-time)\u2026\n\u2713 ready\n" + green) == []
    red = json.dumps([{"name": "kernel-toolchain", "status": "fail", "detail": "no uv and no python3"}])
    problems = yi_usage.kernel_problems(red)
    assert problems == ["kernel-toolchain: no uv and no python3", "kernel-boot: row missing"], problems
    assert yi_usage.kernel_problems("not json")[0].startswith("doctor output is not JSON")


def check_adapter_imports():
    """#317: a stdlib module an adapter names as `mod.attr` is imported at the top; py_compile
    cannot see a NameError, and the one in `write_trajectory` cost two full trials."""
    import ast
    for path in sorted((ROOT / "adapters").rglob("*.py")):
        tree = ast.parse(path.read_text())
        imported = {alias.asname or alias.name.split(".")[0]
                    for node in ast.walk(tree) if isinstance(node, (ast.Import, ast.ImportFrom))
                    for alias in node.names}
        used = {node.value.id for node in ast.walk(tree)
                if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name)}
        missing = sorted(used & {"json", "os", "sys", "re", "time", "subprocess", "shutil", "pathlib"} - imported)
        assert not missing, f"{path.relative_to(ROOT)} uses {missing} without importing"


def check_usage():
    """E5: the parse reads Pi v4 camelCase usage off a real recorded transcript."""
    usage = yi_usage.parse_events(EVENTS)
    assert usage["nAssistantMessages"] == 1, usage
    assert usage["malformedLines"] == 0, usage
    for key in yi_usage.TOKEN_KEYS:
        assert usage[key] == 0, (key, usage[key])
    assert usage["costUsd"] is None, usage
    assert usage.get("byUpstream") == {}, "faux names no upstream"
    # Row 0028 paid twice the catalog on an upstream nothing recorded; the turn's diagnostic names it.
    upstream = {"type": "upstream", "timestamp": 0, "details": {"provider": "Z.AI"}}
    lines = []
    for line in EVENTS.read_text().splitlines():
        event = json.loads(line)
        message = event.get("message") or {}
        if event.get("type") == "message_end" and message.get("role") == "assistant":
            message["diagnostics"] = [{"type": "stream_resent", "timestamp": 0}, upstream]
        lines.append(json.dumps(event))
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "yi.jsonl"
        path.write_text("\n".join(lines) + "\n")
        assert yi_usage.parse_events(path).get("byUpstream") == {"Z.AI": 1}, "the upstream tally"


def check_budget_sentence():
    """S1: every v4 instruction ends `You have 28800 seconds to complete this task.`, the whole
    [agent] timeout; the trial runs the multiplier's share, and the sentence must say that."""
    task = "Fix the filter.\n\nYou have 28800 seconds to complete this task."
    told = yi_usage.with_budget(task, int(yi_usage.TASK_TIMEOUT_SEC * 0.125))
    assert "28800" not in told and told.endswith("You have 3600 seconds to complete this task."), told
    assert yi_usage.with_budget("no budget named", 3600) == "no budget named"
    source = (ROOT / "adapters" / "yi_harbor" / "agent.py").read_text()
    assert "def render_instruction" in source and "with_budget(" in source, "harbor passes the task's 28800 through"


def check_eval_config():
    """A routing A/B is the trial HOME's config and a label in the fingerprint's mode, never a
    rebuild; one helper writes that config for run.py and the harbor adapter."""
    assert yi_usage.eval_config({}) == {"telemetry": {"enabled": True}}
    pinned = {yi_usage.ROUTING_ENV: '{"order": ["deepinfra"], "allow_fallbacks": false}'}
    config = yi_usage.eval_config(pinned)
    assert config["routing"] == {"order": ["deepinfra"], "allow_fallbacks": False}, config
    assert config["telemetry"] == {"enabled": True}, config
    assert yi_usage.routing_label({}) == ""
    assert yi_usage.routing_label(pinned) == '+routing{"allow_fallbacks":false,"order":["deepinfra"]}'
    assert yi_usage.routing_label({yi_usage.ROUTING_ENV: "{}"}) == "+routing{}", "{} sends no provider object"
    for bad in ("not json", '["deepinfra"]'):
        try:
            yi_usage.eval_config({yi_usage.ROUTING_ENV: bad})
        except ValueError:
            continue
        raise AssertionError(f"routing {bad!r} must be refused, not dropped")
    for name in ("run.py", "adapters/yi_harbor/agent.py"):
        source = (ROOT / name).read_text()
        assert "eval_config(" in source and '"enabled"' not in source, f"{name} writes its own config"
        assert "routing_label(" in source, f"{name}'s fingerprint cannot tell two routings apart"
    # The ledger judges a PR's live run by main's runs of the same routing: one pin, three workflows.
    pins = {name: re.findall(r"^\s*EVAL_ROUTING: (.+)$", (ROOT.parent / ".forgejo" / "workflows" / name).read_text(), re.M)
            for name in ("pr.yml", "postmerge.yml", "tracking-hygiene.yml")}
    assert len({pin for found in pins.values() for pin in found}) == 1 and all(pins.values()), pins
    # The fixtures lane leaves a caller's --home config alone, so a label there names a routing never sent.
    with tempfile.TemporaryDirectory() as home:
        done = subprocess.run(
            [sys.executable, str(ROOT / "run.py"), "--home", home, "--binary", str(Path(home) / "no-yi")],
            capture_output=True, text=True, env={**os.environ, yi_usage.ROUTING_ENV: "{}"},
        )
    assert done.returncode == 2 and yi_usage.ROUTING_ENV in done.stderr, (done.returncode, done.stdout)


def check_empty_stream_fails_clean_workspace():
    """Row 0012: a binary that exited 4 before any request scored clean-workspace 1, because the
    runner wrote an empty answer.txt; a rollout that answered nothing leaves no answer file."""
    env = {key: value for key, value in os.environ.items() if key != yi_usage.ROUTING_ENV}
    with tempfile.TemporaryDirectory() as home:
        dead = Path(home) / "yi"
        dead.write_text("#!/bin/sh\nexit 4\n")
        dead.chmod(0o755)
        done = subprocess.run(
            [sys.executable, str(ROOT / "run.py"), "--home", home, "--binary", str(dead), "--task", "clean-workspace"],
            capture_output=True, text=True, env=env, timeout=120,
        )
    assert done.returncode == 0, done.stdout + done.stderr
    row = json.loads(done.stdout.splitlines()[0])
    assert row["reward"] == 0, f"an empty stream scored clean-workspace {row['reward']}"


def check_no_assistant_rows():
    """E9: token fields land all-or-none, never as flattering zeros."""
    kept = []
    for line in EVENTS.read_text().splitlines():
        event = json.loads(line)
        message = event.get("message") or {}
        if event.get("type") == "message_end" and message.get("role") == "assistant":
            continue
        kept.append(line)
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "yi.jsonl"
        path.write_text("\n".join(kept) + "\nnot json at all\n")
        usage = yi_usage.parse_events(path)
    assert usage["nAssistantMessages"] == 0, usage
    assert usage["malformedLines"] == 1, "a corrupt line is counted, never fatal"
    for key in yi_usage.TOKEN_KEYS:
        assert usage[key] is None, (key, usage[key])
    assert usage["costUsd"] is None, usage


def check_unknown_usage_is_not_a_free_turn():
    """D79: a turn whose stream died before its usage chunk prices at zero. If
    the cost path sums that zero, an unmeasurable run walks under the slice cap
    forever, which is the one failure a cap on real money must not have."""
    lines = EVENTS.read_text().splitlines()
    for index, line in enumerate(lines):
        event = json.loads(line)
        message = event.get("message") or {}
        if event.get("type") == "message_end" and message.get("role") == "assistant":
            message["usage"]["unknown"] = True
            lines[index] = json.dumps(event)
    with tempfile.TemporaryDirectory() as directory:
        runs = Path(directory) / "runs" / "trial"
        runs.mkdir(parents=True)
        path = runs / "yi.jsonl"
        path.write_text("\n".join(lines) + "\n")
        usage = yi_usage.parse_events(path)
        assert usage["nAssistantMessages"] == 1, usage
        assert usage["costUnknownTurns"] == 1, usage
        assert usage["costUsd"] is None, "an unreported turn must not read as $0"
        runs = Path(directory) / "runs"
        try:
            total = tb21_cost.spent(runs)
        except tb21_cost.Unmeasurable:
            total = None
        assert total is None, f"unmeasurable spend answered {total} instead of refusing"
        assert tb21_cost.main([str(runs), "--hard", "25"]) == 2, "the probe must stop"
    assert yi_usage.parse_events(EVENTS)["costUnknownTurns"] == 0, "recorded turns are known"


def check_session_extras():
    """Pier's three extra columns, off a committed v4 session fixture."""
    extras = yi_usage.session_extras(SESSIONS)
    assert extras["peak_context_tokens"] == 10410, extras
    assert extras["summarization_count"] == 1, extras
    assert extras["n_agent_steps"] == 1, extras
    with tempfile.TemporaryDirectory() as directory:
        empty = yi_usage.session_extras(directory)
    assert all(value is None for value in empty.values()), empty


def check_fingerprint():
    """The ledger's config column is stable and order-independent."""
    args = ("0.2.0", "anthropic/claude-opus-4-5", "yolo", "tb21@a3f")
    first = yi_usage.config_fingerprint(*args)
    assert first == yi_usage.config_fingerprint(*args), "fingerprint must be stable"
    assert len(first) == 12, first
    other = yi_usage.config_fingerprint("0.2.0", "openai/gpt-5", "yolo", "tb21@a3f")
    assert first != other, "a different model is a different config"
    hour = yi_usage.config_fingerprint("0.2.0", "anthropic/claude-opus-4-5", "yolo+t0.125", "tb21@a3f")
    assert first != hour, "a one-hour row and a full-length row must never share a fingerprint"
    pinned = yi_usage.config_fingerprint("0.2.0", "anthropic/claude-opus-4-5", "yolo", "sha256:39d9f44b")
    assert first != pinned, "a different dataset digest is a different config"


def check_axes():
    """The five-axis scorer reads all three run shapes and its rows are byte-stable."""
    fixture = FIXTURES / "axes"
    expected = (fixture / "expected.jsonl").read_text()
    rows = [json.loads(line) for line in expected.splitlines()]
    # E1: every attempt passed, so the within-task CI is exactly zero; rows 0023 and 0025
    # dropped the falsy 0.0 and printed no width at all.
    both = axes.ledger_row([r for r in rows if r["task"] == "fixture-a"] * 2, "", "", "", "")
    assert "| k=2 pass_at_2=1.0 ±0.0 |" in both, both
    with tempfile.TemporaryDirectory() as directory:
        out = Path(directory) / "rows.jsonl"
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = axes.main([str(fixture), "--json", str(out), "--suite", "fixtures", "--model", "faux/faux-1"])
        assert code == 0, buffer.getvalue()
        assert out.read_text() == expected, "axes rows drifted from evals/fixtures/axes/expected.jsonl"
        row = buffer.getvalue().strip().splitlines()[-1]
    assert "| 1/3 | k=1 |" in row, row
    # E2, E3: fixture-b timed out and its verifier ran past its own ceiling (photonic
    # cc3HUqA); the row counts both and carries the pytest tally and the partial score.
    # fixture-b's one turn ran on Z.AI; the other trials name no upstream.
    assert row.endswith("| 1 / 1(3) / 2 | 3 / 1 / 1 / 1 | 51 / 1 / 17 / 1.0s | 1/1/0 | 12/14 0.35 | Z.AI=1 |"), row
    kinds = sorted(r["kind"] for r in rows)
    assert kinds == ["harbor", "harbor", "journey", "run"], kinds
    with tempfile.TemporaryDirectory() as directory:
        Path(directory, "row.json").write_text(json.dumps({"task": "t", "reward": 0, "exit": 1}))
        assert axes.run_context(Path(directory))["errored"], "a run that exited 1 is not a verified failure"
    # The three timeouts cells never count one trial twice: only the agent's own timeout is `timedOut`,
    # and a verifier that started and raised anything but a timeout (no reward file) is an error alone.
    for exc, cells in (("VerifierTimeoutError", (False, True, False)), ("RewardFileNotFoundError", (False, False, True)),
                       ("AgentSetupTimeoutError", (False, False, True))):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "result.json").write_text(json.dumps({"exception_info": {"exception_type": exc},
                                                                  "verifier": {"started_at": "2026-09-09T00:00:00Z"}}))
            got = axes.harbor_context(Path(directory))
        assert (got["timedOut"], got["verifierUnmeasured"], got["errored"]) == cells, (exc, got)
    # Incident: freight-dispatch-shift scores its trace as `diagnostic_score` (130/232 points) beside
    # a one-test ctrf wrapper that passes; the sweep's partials read 1/1 and no score.
    with tempfile.TemporaryDirectory() as directory:
        trial = Path(directory)
        (trial / "verifier").mkdir()
        (trial / "result.json").write_text(json.dumps({"verifier_result": {"rewards": {"reward": 0.0}}}))
        (trial / "verifier" / "trace_results.json").write_text(json.dumps({"diagnostic_score": 0.5603}))
        assert axes.harbor_context(trial)["partialScore"] == 0.5603, "a trace score under either name"
        # vba-userform-port writes its traces as a list and the tally in trace_summary.json: 0/28
        # beside four wrapper tests that pass.
        (trial / "verifier" / "trace_results.json").write_text(json.dumps([{"name": "001", "ok": False}]))
        (trial / "verifier" / "trace_summary.json").write_text(json.dumps({"passed_traces": 7, "total_traces": 28}))
        assert axes.harbor_context(trial)["partialScore"] == 0.25, "a trace tally in trace_summary.json"
    with tempfile.TemporaryDirectory() as directory:
        empty = Path(directory)
        with contextlib.redirect_stderr(io.StringIO()):
            assert axes.main([str(empty)]) == 2, "no sessions must not read as a measured run"


def check_atif():
    """E10: the session file converts to an ATIF-v1.7 trajectory, byte-stable and harbor-shaped."""
    lines = (FIXTURES / "session-record" / "tool-turn.jsonl").read_text().splitlines()
    trajectory = atif.convert(lines, "0.169.0")
    expected = json.loads((FIXTURES / "atif" / "tool-turn.trajectory.json").read_text())
    assert trajectory == expected, "trajectory drifted from evals/fixtures/atif/tool-turn.trajectory.json"
    assert trajectory["schema_version"] == "ATIF-v1.7"
    steps = trajectory["steps"]
    assert [s["source"] for s in steps] == ["user", "agent", "agent"], steps
    assert steps[1]["metrics"] == {"prompt_tokens": 10200, "completion_tokens": 340, "cached_tokens": 9000, "cost_usd": 0.007432}, steps[1]["metrics"]
    assert [r["source_call_id"] for r in steps[1]["observation"]["results"]] == ["call-1", "call-2"]
    assert trajectory["final_metrics"]["total_steps"] == 3
    for step in steps:
        for call in step.get("tool_calls") or []:
            assert set(call) <= {"tool_call_id", "function_name", "arguments", "extra"}, call
    unknown = [json.dumps({"kind": "header", "id": "u"}), json.dumps({"kind": "entry", "lane": "main", "type": "message",
               "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}], "usage": {"unknown": True}}})]
    assert "total_cost_usd" not in atif.convert(unknown)["final_metrics"], "an unknown-usage turn must not price the trajectory"


def check_line_separator_in_a_string():
    """A raw U+2028 inside a JSON string is one JSONL line; a reader that splits it there drops
    or refuses the entry (#618). serde_json writes U+2028 unescaped, so Yi's files carry it."""
    typed = "run the bash tool twice\u2028and report both exit codes"
    source = (RECORDED / "tool-turn.jsonl").read_text()
    with tempfile.TemporaryDirectory() as directory:
        session = Path(directory) / "sessions" / "tool-turn.jsonl"
        session.parent.mkdir()
        session.write_text(source.replace("run the bash tool twice and report both exit codes", typed))
        try:
            cassette, _ = record.record(session, "t", "d")
            scrubbed = record.scrub_only(session)
        except (ValueError, record.Unrepresentable) as error:
            raise AssertionError(f"record refused the session: {error}") from error
        assert cassette["turns"][0]["user"] == typed, cassette["turns"][0]["user"]
        assert len(scrubbed.split("\n")) == len(source.split("\n")), "scrub_only changed the line count"
        out = Path(directory) / "trajectory.json"
        assert atif.main([str(session), "--out", str(out)]) == 0
        assert json.loads(out.read_text())["steps"][0]["message"] == typed, "atif dropped the user entry"
        run.write_trajectory(session.parent, out, None)
        assert json.loads(out.read_text())["steps"][0]["message"] == typed, "run dropped the user entry"
        events = Path(directory) / "events.jsonl"
        message = {"role": "assistant", "content": [{"type": "text", "text": typed}]}
        events.write_text(json.dumps({"type": "message_end", "message": message}, ensure_ascii=False) + "\n")
        assert yi_arc.last_assistant_text(events) == typed, "yi_arc dropped the final answer"
        lanes = Path(directory) / "lanes.jsonl"
        lanes.write_text(json.dumps({"lane": "text", "haystack": typed, "label": "should-not"}, ensure_ascii=False) + "\n")
        assert rule_fires.load_events(lanes)[0]["haystack"] == typed, "rule_fires cut the event"


def check_driver_ceiling():
    """Both v4 drivers refuse a multiplier past one hour before they need anything installed."""
    for name in ("tbv4_baseline.sh", "tbv4_sweep.sh"):
        script = ROOT / "drivers" / name
        env = {"PATH": os.environ.get("PATH", ""), "TBV4_TIMEOUT_MULT": "0.5"}
        done = subprocess.run(["sh", str(script)], capture_output=True, text=True, env=env, timeout=60,
                              cwd=ROOT.parent)
        assert done.returncode == 1, (name, done)
        assert "0.125 ceiling" in done.stderr, (name, done.stderr)
        env["TBV4_TIMEOUT_MULT"] = "0.125"
        done = subprocess.run(["sh", str(script)], capture_output=True, text=True, env=env, timeout=60,
                              cwd=ROOT.parent)
        assert done.returncode == 1 and "OPENROUTER_API_KEY" in done.stderr, (name, done.stderr)
    # The runner mode (`<runner> <overrides.json> <task>...`, the protocol levers.py calls) files
    # its rows under a run id, so a call without one or without a task is refused before it pays.
    sweep, env = ROOT / "drivers" / "tbv4_sweep.sh", {"PATH": os.environ.get("PATH", "")}
    for argv, said in ((["--runner", "o.json"], "--runner wants"), (["--runner", "o.json", "t"], "EVAL_RUN_ID")):
        done = subprocess.run(["sh", str(sweep), *argv], capture_output=True, text=True, env=env, timeout=60,
                              cwd=ROOT.parent)
        assert done.returncode == 1 and said in done.stderr, (argv, done.stderr)
    # N1: harbor's RuntimeError is both a pull that timed out before the agent ran and an
    # artifact the agent never wrote, so the runner never retries by exception type.
    assert "--retry-include" not in sweep.read_text() and "trials.py unstarted" in sweep.read_text()


def check_watch_stops():
    """The sweep watcher kills its child's group at the wall and writes why; a child that
    finishes first hands back its own exit code."""
    watch = ROOT / "drivers" / "watch.py"
    with tempfile.TemporaryDirectory() as tmp:
        runs = Path(tmp) / "runs"
        done = subprocess.run([sys.executable, str(watch), "--runs", str(runs), "--hard", "20", "--wall", "0",
                               "--poll", "0.2", "--", "sleep", "30"], capture_output=True, text=True, timeout=60)
        assert done.returncode == 2, done
        assert "passed 0 s" in (Path(tmp) / "runs.STOPPED").read_text(), done.stdout
        done = subprocess.run([sys.executable, str(watch), "--runs", str(runs), "--hard", "20", "--wall", "60",
                               "--poll", "0.2", "--", "sh", "-c", "exit 3"], capture_output=True, text=True,
                              timeout=60)
        assert done.returncode == 3, done

    def trial_events(path, cost=None, unknown=False):
        lines = EVENTS.read_text().splitlines()
        for index, line in enumerate(lines):
            event = json.loads(line)
            message = event.get("message") or {}
            if event.get("type") == "message_end" and message.get("role") == "assistant":
                message["usage"]["cost"] = {"total": cost}
                message["usage"]["unknown"] = unknown
                lines[index] = json.dumps(event)
        path.parent.mkdir(parents=True)
        path.write_text("\n".join(lines) + "\n")

    # A trial the watcher stops is marked `censored`: its verifier may still score whatever the
    # stop left, and the gate must not read that as the arm's own result.
    with tempfile.TemporaryDirectory() as tmp:
        runs = Path(tmp) / "runs"
        trial_events(runs / "job" / "task__one" / "agent" / "yi.jsonl", cost=2.0)
        done = subprocess.run([sys.executable, str(watch), "--runs", str(runs), "--hard", "1.5", "--wall", "60",
                               "--poll", "0.2", "--", "sleep", "30"], capture_output=True, text=True, timeout=60)
        assert done.returncode == 2 and "TRIAL STOP task__one" in done.stdout, done
        assert "$1 or 180 turns" in (runs / "job" / "task__one" / "censored").read_text(), "a stopped trial is censored"
    # An unpriced trial is charged the per-trial cap it cannot exceed, never $0 and never a
    # price table (E14), and the stream goes on while the charge fits under the hard cap.
    with tempfile.TemporaryDirectory() as tmp:
        runs = Path(tmp) / "runs"
        trial_events(runs / "job" / "task__two" / "agent" / "yi.jsonl", unknown=True)
        done = subprocess.run([sys.executable, str(watch), "--runs", str(runs), "--hard", "0.5", "--wall", "60",
                               "--poll", "0.2", "--", "sleep", "30"], capture_output=True, text=True, timeout=60)
        assert done.returncode == 2 and "spend $1.000 passed the hard cap" in done.stdout, done


def check_watch_prune():
    """An image goes after IDLE_POLLS consecutive unused polls, a use resets its count, and the
    final prune after harbor exits takes every unused one (PAID-0: a sidecar pruned mid-pull)."""
    import watch
    containers, removed, images = {}, [], ["sidecar", "main"]  # container id -> image id

    def fake_docker(*args):
        if args[0] == "images":
            return "\n".join(f"harborframework/terminal-bench {i}" for i in images if i not in removed)
        if args[0] == "ps":
            return "\n".join(containers)
        return containers.get(args[-1], "")  # inspect --format {{.Image}} <container>

    # PAID-0's sidecar sat unused for the minutes the main image took to pull; at the
    # default 60 s poll, ten polls cover that and two did not.
    assert watch.IDLE_POLLS >= 10, watch.IDLE_POLLS
    real_docker, real_run = watch.docker, watch.subprocess.run
    watch.docker = fake_docker
    watch.subprocess.run = lambda argv, **kw: removed.append(argv[-1]) or subprocess.CompletedProcess(argv, 0)
    try:
        idle = {}
        for _ in range(watch.IDLE_POLLS - 1):
            watch.prune(idle)
        assert removed == [], "an image unused for fewer than IDLE_POLLS polls stays"
        containers["c1"] = "main"  # the main image's container starts: its count resets
        watch.prune(idle)
        assert removed == ["sidecar"], removed
        del containers["c1"]
        watch.prune(idle)
        assert removed == ["sidecar"], "main was used one poll ago"
        assert watch.prune(idle, ripe_at=0) == 1 and removed == ["sidecar", "main"], "the final prune takes every unused image"
    finally:
        watch.docker, watch.subprocess.run = real_docker, real_run

def check_trials():
    """One row per harbor trial, whatever sessions it wrote, and the caps a runner call may spend."""
    import trials
    with tempfile.TemporaryDirectory() as tmp:
        job = Path(tmp) / "job"
        shutil.copytree(FIXTURES / "axes" / "harbor" / "job", job)
        # A child session beside the root one: the trial's tokens and cost are the sum of both.
        root = next((job / "fixture-a__x" / "agent" / "yi" / "sessions").glob("*.jsonl"))
        shutil.copy(root, root.with_name("1787544431470_fixture-a-child.jsonl"))
        (job / "fixture-b__y" / "censored").write_text("stopped\n")
        rows = {row["trial"]: row for row in trials.trial_rows(job)}
        assert sorted(rows) == ["fixture-a__x", "fixture-b__y"], rows
        assert (rows["fixture-a__x"]["input"], rows["fixture-a__x"]["costUsd"]) == (2400, 0.014864), rows["fixture-a__x"]
        assert rows["fixture-b__y"]["censored"] and not rows["fixture-a__x"]["censored"], rows
        for key in ("task", "reward", "partialScore", "cacheRead", "output", "wallSec", "errored",
                    "testsPassed", "testsTotal", "traceScored"):
            assert key in rows["fixture-a__x"], key
        assert rows["fixture-b__y"]["traceScored"] and not rows["fixture-a__x"]["traceScored"], rows
        # A trial that finished without a session ran no agent: its task is the one rerun.
        unstarted = job / "fixture-c__z"
        unstarted.mkdir()
        (unstarted / "result.json").write_text(json.dumps({"task_name": "terminal-bench/fixture-c",
                                                            "exception_info": {"exception_type": "RuntimeError"}}))
        assert trials.unstarted(job) == ["fixture-c"], trials.unstarted(job)
        store = Path(tmp) / "store"
        store.mkdir()
        real_store, trials.STORE = trials.STORE, store
        try:
            with contextlib.redirect_stdout(io.StringIO()) as said:
                assert trials.main(["rows", str(job), "--run-id", "r", "--dry"]) == 0
            assert len(said.getvalue().splitlines()) == 2 and not list(store.iterdir()), "--dry prints and files nothing"
        finally:
            trials.STORE = real_store
    now = 1790000000  # 2026-09-21T14:13:20Z, a Monday
    with tempfile.TemporaryDirectory() as tmp:
        store = Path(tmp)
        def put(run, *costs, at=now):
            with (store / f"{run}.jsonl").open("a") as sink:
                for cost in costs:
                    sink.write(json.dumps({"task": "t", "costUsd": cost, "at": at}) + "\n")
        put("old", 50.0, at=now - 8 * 86400)
        put("r1", 20.0, None)
        assert trials.spend(store, now) == (0.0, 21.0), "last week is not this week; an unpriced trial costs $1"
        assert trials.caps("r2", 12, store, now) == (9.0, None), "hard: the smaller of the stage's $15 and the week's $30 left"
        assert trials.caps("r2", 16, store, now)[1].startswith("week soft cap"), "21 + 16 x 0.27 passes the $25 soft cap"
        put("r2", 12.9)
        assert trials.caps("r2", 1, store, now)[1].startswith("stage soft cap"), "12.9 + 0.27 passes the stage's $13"


def check_pin_probe():
    """N4 (design 6.4): pin the cheapest upstream whose every warm turn hit at least 0.9 and that
    returned no 429; with none, the best hit rate with fallbacks allowed, and the verdict says so."""
    def sample(t2_input, t2_read, cost=0.001, error=None):
        return {"t2": {"input": t2_input, "cacheRead": t2_read, "cacheWrite": 0, "costUsd": cost,
                       "nAssistantMessages": 1, "costUnknownTurns": 0}, "error": error}
    probes = {"together": [sample(100, 9900, 0.002)] * 3,
              "parasail": [sample(500, 9500, 0.001)] * 3,
              "relace": [sample(100, 9900, 0.0005), sample(100, 9900, 0.0005), sample(100, 9900, error="HTTP 429")],
              "wafer": [sample(9000, 1000, 0.0001)] * 3}
    verdict = cache_probe.pin(probes)
    assert verdict["pin"] == "parasail" and verdict["allowFallbacks"] is False, verdict
    assert verdict["routing"] == {"order": ["parasail"], "allow_fallbacks": False}, verdict
    assert verdict["upstreams"]["relace"]["qualifies"] is False, "a 429 disqualifies however cheap"
    assert verdict["upstreams"]["together"]["qualifies"] is True, verdict
    # The first session on an upstream writes the cache the later ones read (N4): a cold first
    # sample does not disqualify, a cold later one does.
    warm = cache_probe.pin({"z-ai": [sample(13273, 0)] + [sample(345, 12928)] * 2})
    assert (warm["pin"], warm["allowFallbacks"]) == ("z-ai", False), warm
    cold = cache_probe.pin({"relace": [sample(13280, 0), sample(13273, 0), sample(342, 12928)]})
    assert cold["allowFallbacks"] is True, cold
    none = cache_probe.pin({"wafer": [sample(9000, 1000)] * 3, "together": [sample(5000, 5000)] * 3})
    assert (none["pin"], none["allowFallbacks"]) == ("together", True), none
    assert none["routing"] == {"order": ["together"], "allow_fallbacks": True}, none


def check_record():
    """J3: a recorder that regroups turns, reorders tool-call arguments,
    reorders stub results or invents zero usage writes a cassette that replays
    a session which never happened."""
    cassette, notes = record.record(RECORDED / "tool-turn.jsonl", "t", "d")
    assert len(cassette["turns"]) == 1, cassette["turns"]
    turn = cassette["turns"][0]
    assert turn["user"].startswith("run the bash tool twice"), turn["user"]
    assert len(turn["responses"]) == 2, turn["responses"]

    call, closing = turn["responses"]
    assert call["text"] == "running the tool", call
    assert [c["id"] for c in call["toolCalls"]] == ["call-1", "call-2"], call
    arguments = call["toolCalls"][0]["arguments"]
    assert list(arguments) == ["zeta", "alpha", "nested"], list(arguments)
    assert list(arguments["nested"]) == ["b", "a"], list(arguments["nested"])
    assert arguments["nested"]["a"] == [1, None, True], arguments
    assert (call["usageTotal"], call["usageInput"]) == (10750, 1200), call
    assert (closing["usageTotal"], closing["usageInput"]) == (12112, 1400), closing
    assert "toolCalls" not in closing, closing

    results = cassette["stubs"][0]["results"]
    assert cassette["stubs"][0]["name"] == "bash", cassette["stubs"]
    assert [r["text"] for r in results] == ["exit 0", "exit 1: false"], results
    assert [r["isError"] for r in results] == [False, True], results
    assert any("thinking" in note for note in notes), notes


def check_record_redacts():
    """J3 §10: a credential that survives recording ships into a committed
    cassette, and a $HOME path makes the artifact machine-specific."""
    cassette, notes = record.record(RECORDED / "faux-echo.jsonl", "t", "d")
    blob = json.dumps(cassette)
    assert PLANT not in blob, "planted secret survived recording"
    assert "[MASKED]" in blob, "the plant was not masked; the fixture proves nothing"
    assert "CASSETTE-NEEDLE-7" in blob, "redaction swallowed the prompt's own content"
    home = str(Path.home())
    assert home not in blob, f"recorded cassette carries {home}"
    assert any("custom" in note for note in notes), notes

    source = RECORDED / "faux-echo.jsonl"
    scrubbed = record.scrub_only(source)
    assert PLANT not in scrubbed, "planted secret survived --scrub-only"
    assert "[MASKED]" in scrubbed, "the scrub-only path masked nothing"
    assert "CASSETTE-NEEDLE-7" in scrubbed, "redaction swallowed the session's own content"
    assert home not in scrubbed, f"scrubbed session carries {home}"
    assert len(scrubbed.splitlines()) == len(source.read_text().splitlines()), (
        "--scrub-only writes every line back, in order"
    )
    assert json.loads(scrubbed.splitlines()[0])["kind"] == "header", "the header stays a header"


def check_record_refuses():
    """J3: unrepresentable input is fatal, never a silent skip — a dropped
    entry would emit a cassette that replays a session that never happened."""
    header = '{"kind":"header","version":4,"id":"x","createdAt":1,"cwd":"/tmp"}'
    user = (
        '{"kind":"entry","lane":"main","type":"message","id":"e1","seq":1,'
        '"message":{"role":"user","content":"hi","timestamp":1}}'
    )
    unrepresentable = {
        "compaction rewrites the context": (
            '{"kind":"entry","lane":"main","type":"compaction","id":"e2","seq":2,'
            '"summary":"gone","tokensBefore":9}'
        ),
        "a lane the cassette cannot replay": (
            '{"kind":"entry","lane":"thread","type":"message","id":"e2","seq":2,'
            '"message":{"role":"user","content":"hi","timestamp":1}}'
        ),
        "a role the schema lacks": (
            '{"kind":"entry","lane":"main","type":"message","id":"e2","seq":2,'
            '"message":{"role":"compactionSummary","summary":"s","timestamp":1}}'
        ),
        "a corrupt line": "{not json",
    }
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "s.jsonl"
        for reason, line in unrepresentable.items():
            path.write_text("\n".join((header, user, line)) + "\n")
            try:
                record.record(path, "t", "d")
            except record.Unrepresentable:
                continue
            raise AssertionError(f"{reason}: recorded instead of refusing")
        path.write_text(header + "\n")
        try:
            record.record(path, "t", "d")
        except record.Unrepresentable:
            return
        raise AssertionError("a session with no user message must refuse")


def check_cost_cap():
    """The campaign slice's only automated money gate: the probe's exit code
    stops the driver, and a spend it cannot compute must never read as $0."""
    priced = (
        '{"type":"message_end","message":{"role":"assistant",'
        '"usage":{"cost":{"total":26.0}}}}'
    )
    with tempfile.TemporaryDirectory() as directory:
        runs = Path(directory)
        assert tb21_cost.main([str(runs), "--soft", "20", "--hard", "25"]) == 0
        (runs / "trial").mkdir()
        (runs / "trial" / "yi.jsonl").write_text(priced + "\n")
        assert tb21_cost.main([str(runs), "--hard", "25"]) == 2, "hard cap slept"
        assert tb21_cost.main([str(runs), "--soft", "20"]) == 2, "soft cap slept"
        # A task that ran and left no transcript prices at nothing, which is not $0.
        assert tb21_cost.main([str(runs), "--min-files", "1"]) == 0, "one transcript, one expected"
        assert tb21_cost.main([str(runs), "--min-files", "2"]) == 2, "a missing transcript walked under the cap"

    def unreadable(_runs_dir, _min_files=0):
        raise OSError("the runs directory could not be read")

    original, tb21_cost.spent = tb21_cost.spent, unreadable
    try:
        tb21_cost.main([".", "--hard", "25"])
        raise AssertionError("a probe that cannot price the run must not exit 0")
    except OSError:
        pass
    finally:
        tb21_cost.spent = original


def check_orient_census():
    """P4/P13 census: row counts, escalation labels, the layer parse, no leak."""
    orient_census.selftest()


def check_rule_fires():
    """The corpus is labelled; the engine that reads it is pinned in rules_e2e."""
    report = rule_fires.measure(FIXTURES / "rules" / "lanes.jsonl")
    assert report["should"] == 2, report
    assert report["should_not"] == 6, report
    assert report["recall_oracle"] == 1.0, report
    assert report["comment_fp"] == 2, report


def check_surface_compare():
    """N6 (design 6.6): a tool-text patch is refused when any tool's refusal rate rises or a
    scenario clean on every base run turns unclean; a candidate no worse passes."""
    def doc(calls, refusals, rows):
        return {"toolSurface": {"calls": calls, "refusals": refusals}, "rows": rows}
    clean = {"scenario": "write-then-edit", "exit": 0, "timedOut": False, "missingFiles": []}
    base = [doc({"edit": 10, "read": 5}, {"edit": 2}, [clean])] * 2
    assert surface.compare(base, [doc({"edit": 10, "read": 5}, {"edit": 1}, [clean])] * 2) is None
    assert surface.compare(base, [doc({"edit": 10}, {"edit": 3}, [clean])] * 2) == "refusal_rate_rose:edit"
    assert surface.compare(base, [doc({"read": 4}, {"read": 1}, [clean])] * 2) == "refusal_rate_rose:read"
    lost = dict(clean, missingFiles=["stats.py"])
    assert surface.compare(base, [doc({"edit": 10}, {}, [clean]), doc({"edit": 10}, {}, [lost])]) \
        == "scenario_unclean:write-then-edit"


def check_surface():
    """The tool-surface loop's schema and its census, with no binary and no key:
    every scenario parses, a malformed one is refused by name, and the per-tool
    counts come off the extractor's own store."""
    assert surface.main(["--selfcheck", "--model", "faux/faux-1"]) == 0
    roads = {scenario["id"]: scenario["road"] for scenario in surface.load_scenarios()}
    assert len(roads) >= 6, roads
    with tempfile.TemporaryDirectory() as directory:
        broken = Path(directory) / "scenarios.json"
        broken.write_text(json.dumps({"scenarios": [{"id": "x", "road": "r", "prompt": "p"}]}))
        try:
            surface.load_scenarios(broken)
            raise AssertionError("a scenario missing timeoutSec, seed and clean was admitted")
        except ValueError as error:
            assert "missing" in str(error), error
        store = Path(directory) / "mining"
        store.mkdir()
        (store / "mu.jsonl").write_text(json.dumps(
            {"sessionId": "s1", "file": "a.jsonl", "toolCalls": {"byTool": {"edit": 4, "bash": 1}}}) + "\n")
        (store / "issues.jsonl").write_text(json.dumps(
            {"tool": "edit", "count": 3, "state": "NEW", "sessions": ["s1"],
             "example": "hash #A1B2 is not from this session", "resolution": {"pivot": 1, "unresolved": 2}}) + "\n")
        counts = surface.census(store)
        assert counts["calls"] == {"bash": 1, "edit": 4}, counts
        assert counts["refusals"] == {"edit": 3} and counts["rate"]["edit"] == 0.75, counts
        text = surface.report([], store, Path(directory))
        # The verdict is a human's; the runner prints the blank and never fills it.
        assert "correct? ____" in text and "recovered 1, unresolved 2" in text, text


def check_graph_refiner():
    """D219: the refiner's gates on synthetic rows, and the fixture Rust judges too."""
    report = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_refine)
    result = unittest.TextTestRunner(stream=report).run(suite)
    assert result.testsRun >= 5 and result.wasSuccessful(), report.getvalue()


def check_improve():
    """The round's proposer half: a development-only corpus, a history-free snapshot, S0."""
    report = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_improve)
    result = unittest.TextTestRunner(stream=report).run(suite)
    assert result.testsRun >= 4 and result.wasSuccessful(), report.getvalue()

def check_levers():
    """D220: the manifest, the shared default fixture and the floors agree, and the gates hold."""
    report = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_levers)
    result = unittest.TextTestRunner(stream=report).run(suite)
    assert result.testsRun >= 5 and result.wasSuccessful(), report.getvalue()
    environ = {yi_usage.LEVERS_ENV: str(ROOT / "levers" / "default.json")}
    assert yi_usage.levers_label({}) == "" and len(yi_usage.levers_label(environ)) == len("+levers") + 12


def check_skill_labels():
    """#779: only typed messages enter the classifier's corpus, scrubbed and once; the teacher
    run resumes and stops at its budget; the frozen sample is stratified for the owner."""
    report = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_skill_labels)
    result = unittest.TextTestRunner(stream=report).run(suite)
    assert result.testsRun >= 4 and result.wasSuccessful(), report.getvalue()


def check_judge_replay():
    """D259: the readers on recorded transcripts, blindness, the direct call, quote bytes, the caps
    and the metrics."""
    report = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromModule(test_judge_replay)
    result = unittest.TextTestRunner(stream=report).run(suite)
    assert result.testsRun >= 20 and result.wasSuccessful(), report.getvalue()


CHECKS = (
    check_surface,
    check_surface_compare,
    check_judge_replay,
    check_graph_refiner,
    check_levers,
    check_skill_labels,
    check_improve,
    check_trials,
    check_watch_prune,
    check_pin_probe,
    check_cost_cap,
    check_orient_census,
    check_rule_fires,
    check_command,
    check_install,
    check_adapter_imports,
    check_usage,
    check_budget_sentence,
    check_eval_config,
    check_empty_stream_fails_clean_workspace,
    check_no_assistant_rows,
    check_unknown_usage_is_not_a_free_turn,
    check_session_extras,
    check_fingerprint,
    check_driver_ceiling,
    check_watch_stops,
    check_axes,
    check_atif,
    check_line_separator_in_a_string,
    check_record,
    check_record_redacts,
    check_record_refuses,
)


def main():
    errors = []
    for check in CHECKS:
        try:
            check()
        except AssertionError as error:
            errors.append(f"{check.__name__}: {error}")
    # One line per gate: check_guardrails.sh runs this beside the python gates
    # and reads only the exit code, so the output matches _common.fail's shape.
    if errors:
        print("FAIL evals_selftest")
        for error in errors:
            print(f"  {error}")
        return 1
    print(f"ok   evals_selftest ({len(CHECKS)} checks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
