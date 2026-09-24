"""Who a failed tool result blames: the tool, the model's command, or its call into rlm/yi."""

import json
import re
from pathlib import Path

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
EXIT_RE = re.compile(r"^exit code: -?\d+", re.M)
# A bash result the tool refused before a shell ran carries no exit code.
BASH_REFUSAL_RE = re.compile(r"^(?:\[refused:|Denied by |Permission denied: command )")
# IPython heads a traceback with the class name; a SyntaxError only ends with `Name: value`.
ENAME_RE = re.compile(r"^(\w+) +Traceback \(most recent call last\)|^(\w*(?:Error|Exception)): ", re.M)
# The raising package, as the kernel venv installs it.
API_FRAME_RE = re.compile(r"site-packages/(?:rlm|yi)/")
# Invariant: mirrors the exception classes python/yi_runtime/src/yi and rlm export.
API_ERRORS = frozenset(
    "PlanError Refused Stale SpecDrift RunActive McpToolError McpStartupError".split()
)
API_NAME_RE = re.compile(
    r"module '(?:yi|rlm)'|from '(?:yi|rlm)'|No module named '(?:yi|rlm)\b"
    r"|'(?:Plan|Todo|Run|Writer|Reader|Contract|_RLMCallable)'"
    r"|\b(?:Plan|Todo|Run|Writer|Reader|Contract)(?:\.\w+)?\(\)"
)


def classify(tool, text, details=None):
    details = details if isinstance(details, dict) else {}
    text = ANSI_RE.sub("", text or "")
    # The closed errorKind taxonomy is only ever set by the tool about itself.
    if details.get("errorKind"):
        return "tool"
    if tool == "bash":
        if details.get("timedOut") or BASH_REFUSAL_RE.match(text):
            return "tool"
        if details.get("exitCode") not in (None, 0) or EXIT_RE.search(text):
            return "command"
        return "other"
    if tool != "ipython":
        return "tool"
    error = details.get("error") if isinstance(details.get("error"), dict) else {}
    trace = ANSI_RE.sub("", "\n".join(error.get("traceback") or [])) or text
    found = ENAME_RE.search(trace)
    ename = error.get("ename") or (found and (found.group(1) or found.group(2)))
    if not ename:
        return "other"
    evalue = str(error.get("evalue") or "")
    if not evalue:
        tail = re.search(rf"^{re.escape(ename)}: (.*)", trace, re.M)
        evalue = tail.group(1) if tail else ""
    if ename in API_ERRORS or API_FRAME_RE.search(trace):
        return "api_misuse"
    if ename in ("TypeError", "AttributeError", "ImportError", "ModuleNotFoundError"):
        if API_NAME_RE.search(evalue):
            return "api_misuse"
    return "command"


def selfcheck(fixture):
    rows = [json.loads(line) for line in Path(fixture).read_text().splitlines() if line]
    wrong = [
        (row["label"], got, row["note"])
        for row in rows
        if (got := classify(row["tool"], row["text"], row["details"])) != row["label"]
    ]
    assert not wrong, f"error classes mislabelled (want, got, case): {wrong}"
    missing = {"tool", "command", "api_misuse"} - {row["label"] for row in rows}
    assert not missing, f"labelled fixture has no {sorted(missing)} case"
    return rows


def session(labelled, directory):
    """The labelled cases as one v4 session, so a sweep proves the class reaches the store."""
    rows = [{"kind": "header", "version": 4, "id": "fixture-ec", "createdAt": 1780000000000}]
    for i, case in enumerate(labelled):
        call = {"type": "toolCall", "id": f"e{i}", "name": case["tool"], "arguments": {"n": i}}
        rows.append({"type": "message", "message": {"role": "assistant", "content": [call]}})
        result = {"role": "toolResult", "toolCallId": f"e{i}", "toolName": case["tool"], "isError": True}
        result.update(content=[{"type": "text", "text": case["text"]}], details=case["details"])
        rows.append({"type": "message", "message": result})
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "ec.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    return directory
