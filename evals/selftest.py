#!/usr/bin/env python3
"""Dry-run gate for the Yi eval adapters: no docker, no API key, no harness.

Each check defends one gate from YI_DESIGN 15.2 against a recorded artifact,
so a broken adapter fails here instead of scoring a real rollout 0.

    python3 evals/selftest.py
"""

import json
import shlex
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT))

import record  # noqa: E402
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
    assert "grep" not in command, "E6: a filtered stream exits 1 under pipefail"
    assert argv.count(INSTRUCTION) == 1, "E7: instruction must be one quoted argv"
    assert yi_usage.REMOTE_SESSION_DIR in argv, "session dir must be collected"
    assert yi_usage.REMOTE_SESSION_DIR.startswith("/logs/"), "sessions live under /logs"
    assert "--continue" not in argv, "resume is opt-in"
    assert "--continue" in shlex.split(
        yi_usage.run_command("anthropic/claude-opus-4-5", "hi", resume=True)
    ), "resume must pass --continue"
    for bad in ("", "claude-opus-4-5"):
        try:
            yi_usage.run_command(bad, "hi")
        except ValueError:
            continue
        raise AssertionError(f"model name {bad!r} must be rejected")


def check_usage():
    """E5: the parse reads Pi camelCase usage off a real recorded transcript."""
    usage = yi_usage.parse_events(EVENTS)
    assert usage["nAssistantMessages"] == 1, usage
    assert usage["malformedLines"] == 0, usage
    for key in yi_usage.TOKEN_KEYS:
        assert usage[key] == 0, (key, usage[key])
    assert usage["costUsd"] is None, usage


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


CHECKS = (
    check_command,
    check_usage,
    check_no_assistant_rows,
    check_session_extras,
    check_fingerprint,
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
