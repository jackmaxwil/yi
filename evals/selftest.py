#!/usr/bin/env python3
"""Dry-run gate for the Yi eval adapters: no docker, no API key, no harness.

Each check defends one gate from YI_DESIGN 15.2 against a recorded artifact,
so a broken adapter fails here instead of scoring a real rollout 0.

    python3 evals/selftest.py
"""

import contextlib
import io
import json
import os
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT / "drivers"))
sys.path.insert(0, str(ROOT))

import orient_census  # noqa: E402
import rule_fires  # noqa: E402
import record  # noqa: E402
import tb21_cost  # noqa: E402
import atif  # noqa: E402
import axes  # noqa: E402
import yi_usage  # noqa: E402

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
    # E6: a bare `grep -v` exits 1 when nothing survives and pipefail scores the
    # trial 0; the delta filter is guarded, and tee's stdout is not harbor's to hold.
    assert "grep -v" in command and "|| [ $? -eq 1 ]" in command, "E6: the delta filter must be guarded"
    assert command.rstrip().endswith(">/dev/null"), "harbor must not buffer the event stream"
    assert "message_update" in command, "the deltas are what made one trial 43.8 GB"
    assert f"tee -a {yi_usage.REMOTE_EVENTS_PATH}" in command, "a resumed trial truncated its first segment"
    assert argv.count(INSTRUCTION) == 1, "E7: instruction must be one quoted argv"
    assert yi_usage.REMOTE_SESSION_DIR in argv, "session dir must be collected"
    assert yi_usage.REMOTE_SESSION_DIR.startswith("/logs/"), "sessions live under /logs"
    assert "--continue" not in argv, "resume is opt-in"
    assert "--continue" in shlex.split(
        yi_usage.run_command("anthropic/claude-opus-4-5", "hi", resume=True)
    ), "resume must pass --continue"
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
    """E5: the parse reads Pi camelCase usage off a real recorded transcript."""
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
    # The fixtures lane leaves a caller's --home config alone, so a label there names a routing never sent.
    with tempfile.TemporaryDirectory() as home:
        done = subprocess.run(
            [sys.executable, str(ROOT / "run.py"), "--home", home, "--binary", str(Path(home) / "no-yi")],
            capture_output=True, text=True, env={**os.environ, yi_usage.ROUTING_ENV: "{}"},
        )
    assert done.returncode == 2 and yi_usage.ROUTING_ENV in done.stderr, (done.returncode, done.stdout)


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


def check_driver_ceiling():
    """The v4 driver refuses a multiplier past one hour before it needs anything installed."""
    script = ROOT / "drivers" / "tbv4_baseline.sh"
    env = {"PATH": os.environ.get("PATH", ""), "TBV4_TIMEOUT_MULT": "0.5"}
    done = subprocess.run(["sh", str(script)], capture_output=True, text=True, env=env, timeout=60)
    assert done.returncode == 1, done
    assert "0.125 ceiling" in done.stderr, done.stderr
    env["TBV4_TIMEOUT_MULT"] = "0.125"
    done = subprocess.run(["sh", str(script)], capture_output=True, text=True, env=env, timeout=60)
    assert done.returncode == 1 and "OPENROUTER_API_KEY" in done.stderr, done.stderr


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


CHECKS = (
    check_cost_cap,
    check_orient_census,
    check_rule_fires,
    check_command,
    check_install,
    check_adapter_imports,
    check_usage,
    check_budget_sentence,
    check_eval_config,
    check_no_assistant_rows,
    check_unknown_usage_is_not_a_free_turn,
    check_session_extras,
    check_fingerprint,
    check_driver_ceiling,
    check_axes,
    check_atif,
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
