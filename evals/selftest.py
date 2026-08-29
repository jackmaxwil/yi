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

import yi_usage  # noqa: E402

FIXTURES = ROOT / "fixtures"
EVENTS = FIXTURES / "ask_events.jsonl"
SESSIONS = FIXTURES / "session"

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


CHECKS = (
    check_command,
    check_usage,
    check_no_assistant_rows,
    check_session_extras,
    check_fingerprint,
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
