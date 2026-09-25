"""Shared, stdlib-only logic for the Yi harbor and pier adapters.

Pure functions only: no harness imports, so evals/selftest.py exercises the
command contract and the usage parse without harbor, pier, docker, or keys.
Contracts: evals/README.md (gates E1-E9, emission map, adapters).
"""

import hashlib
import json
import shlex
from pathlib import Path

ADAPTER_VERSION = 1

# Remote paths inside the trial container. harbor/pier sync /logs back to the
# host, where logs_dir/EVENTS_FILENAME is the transcript the parse reads.
REMOTE_SESSION_DIR = "/logs/agent/yi/sessions"
REMOTE_EVENTS_PATH = "/logs/agent/yi.jsonl"
# logs_dir on the host is /logs/agent in the container, so these are the same
# two artifacts seen from the two ends of the log sync.
EVENTS_FILENAME = "yi.jsonl"
SESSIONS_SUBDIR = "yi/sessions"

# E5: the Pi v4 session format spells usage in camelCase. One definition, both parsers -- a
# snake_case slip here reads every column as zero instead of failing loudly.
TOKEN_KEYS = ("input", "output", "cacheRead", "cacheWrite")


# Every Terminal-Bench v4 task's [agent] timeout; the trial's share is the multiplier's.
TASK_TIMEOUT_SEC = 28800

# OpenRouter's provider object for a routing A/B, as JSON; never `YI_*`, the harness's name.
ROUTING_ENV = "EVAL_ROUTING"
LEVERS_ENV = "YI_LEVERS"


def with_budget(instruction, deadline_sec):
    """S1: every v4 instruction ends `You have 28800 seconds to complete this task.`, the whole
    [agent] timeout, while the trial runs the multiplier's share: the sentence names that share."""
    return instruction.replace(
        f"You have {TASK_TIMEOUT_SEC} seconds", f"You have {int(deadline_sec)} seconds"
    )


def eval_config(environ):
    """The trial HOME's `~/.yi/config.json` for run.py and the harbor adapter: telemetry on
    (D132), and `routing` verbatim from EVAL_ROUTING, refused when it is not a JSON object."""
    config = {"telemetry": {"enabled": True}}
    raw = environ.get(ROUTING_ENV)
    if raw:
        try:
            routing = json.loads(raw)
        except ValueError:
            routing = None
        if not isinstance(routing, dict):
            raise ValueError(f"{ROUTING_ENV} must be a JSON object, not {raw!r}")
        config["routing"] = routing
    return config


def routing_label(environ):
    """The routing object as it rides the fingerprint's mode; `{}` (no provider object) is a
    variant of its own, and no EVAL_ROUTING is no label."""
    routing = eval_config(environ).get("routing")
    if routing is None:
        return ""
    return "+routing" + json.dumps(routing, sort_keys=True, separators=(",", ":"))


def levers_label(environ):
    """The override's hash as it rides the fingerprint's mode (plan section 10.6): two runs
    under different levers are different configs, and two under the same levers are one
    config however the file was spelled, so the hash is of the canonical object. The binary
    reads the file, never this."""
    path = environ.get(LEVERS_ENV)
    if not path:
        return ""
    with open(path, "rb") as handle:
        canonical = json.dumps(json.loads(handle.read()), sort_keys=True, separators=(",", ":"))
    return "+levers" + hashlib.sha256(canonical.encode("utf-8")).hexdigest()[:12]


KERNEL_ROWS = ("kernel-toolchain", "kernel-boot")


def kernel_problems(doctor_json):
    """The kernel rows of `yi doctor --json` that are red, as `name: detail` lines; the
    adapter refuses an image the kernel cannot boot on instead of running bash-only.
    harbor hands the adapter one combined stream, so `--fix`'s progress lines (stderr)
    precede the rows: the array is read from the first line that opens one."""
    text = str(doctor_json or "")
    start = next((i for i, line in enumerate(text.splitlines()) if line.startswith("[")), None)
    try:
        rows = json.loads("\n".join(text.splitlines()[start:])) if start is not None else None
    except ValueError:
        rows = None
    if not isinstance(rows, list):
        return [f"doctor output is not JSON: {text[:200]!r}"]
    by_name = {row.get("name"): row for row in rows if isinstance(row, dict)}
    problems = []
    for name in KERNEL_ROWS:
        row = by_name.get(name)
        if row is None:
            problems.append(f"{name}: row missing")
        elif row.get("status") not in ("ok", "fixed"):
            problems.append(f"{name}: {row.get('detail')}")
    return problems


def run_command(model_name, instruction, resume=False, deadline_sec=None):
    """Build the single shell command a trial runs.

    E6: no `grep` stage -- under harbor's `set -o pipefail` a fully filtered
    stream exits 1 and scores the trial 0. E7: the instruction is one shell
    quoted argv. E8: `--yolo`, because a permission prompt is a hang. `--here`:
    the grader reads the task's checkout, and an ask in a repository takes a
    lane outside it (D119).
    """
    if not model_name or "/" not in model_name:
        raise ValueError("model name must be 'provider/model'")
    resume_flag = "--continue " if resume else ""
    deadline_flag = f"--deadline {int(deadline_sec)} " if deadline_sec else ""
    # Incident: a 131k-token turn streamed 130,496 `message_update` snapshots,
    # 43.8 GB on one trial; harbor buffers the command's stdout, and the OS
    # killed it. The deltas are dropped through a guarded filter (E6: a bare
    # `grep -v` exits 1 when nothing survives, and pipefail scores that 0),
    # message_end carries every field the parse reads, and tee's stdout goes
    # to /dev/null so harbor holds nothing in memory. `-a`: a resumed trial's
    # `--continue` run appends to the first segment instead of truncating it.
    # Incident: grep block-buffers into a pipe, so a killed trial's file ended
    # mid-event at 172,032 bytes; `stdbuf -oL`, since busybox grep has no
    # `--line-buffered`.
    return (
        "yi ask --json --yolo --here "
        f"--model {shlex.quote(model_name)} "
        f"--session-dir {REMOTE_SESSION_DIR} "
        f"{deadline_flag}{resume_flag}{shlex.quote(instruction)} "
        "2>&1 </dev/null | { stdbuf -oL grep -v '\"type\":\"message_update\"' || [ $? -eq 1 ]; } "
        f"| stdbuf -oL tee -a {REMOTE_EVENTS_PATH} >/dev/null"
    )


def _add_tokens(totals, usage):
    if not isinstance(usage, dict):
        return
    for key in TOKEN_KEYS:
        value = usage.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            totals[key] += value


def json_lines(path):
    """Yield parsed objects and a malformed count; a bad line never aborts."""
    malformed = 0
    parsed = []
    try:
        handle = Path(path).open(errors="replace")
    except OSError:
        return [], 0
    # Streamed, never read whole: the cost probe was killed reading a 43 GB file.
    with handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                malformed += 1
                continue
            if isinstance(value, dict):
                parsed.append(value)
            else:
                malformed += 1
    return parsed, malformed


def upstreams(messages):
    """Turns per upstream, from the `upstream` diagnostic the completions mapper keeps off
    OpenRouter's chunks; a turn that names none (faux, a direct provider) is not counted."""
    tally = {}
    for message in messages:
        for note in message.get("diagnostics") or []:
            name = (note.get("details") or {}).get("provider") if note.get("type") == "upstream" else None
            if name:
                tally[name] = tally.get(name, 0) + 1
    return tally


def parse_events(path):
    """Sum assistant usage over a `yi ask --json` transcript.

    E9: all token fields land together or not at all -- with no assistant
    `message_end` row every field stays None, because AA drops missing values
    from averages and a partial zero flatters the run silently.

    D79's `usage.unknown` is the same hazard one turn at a time: a stream that
    died before its usage chunk reports zeros that are forged, not measured. A
    run carrying any such turn has no total, so `costUsd` is None and
    `costUnknownTurns` names how many turns the token columns under-count.
    """
    totals = {key: 0 for key in TOKEN_KEYS}
    cost = 0.0
    assistant = 0
    unknown = 0
    turns = []
    events, malformed = json_lines(path)
    for event in events:
        if event.get("type") != "message_end":
            continue
        message = event.get("message")
        if not isinstance(message, dict) or message.get("role") != "assistant":
            continue
        assistant += 1
        turns.append(message)
        usage = message.get("usage")
        _add_tokens(totals, usage)
        if isinstance(usage, dict) and usage.get("unknown") is True:
            unknown += 1
        if isinstance(usage, dict) and isinstance(usage.get("cost"), dict):
            total = usage["cost"].get("total")
            if isinstance(total, (int, float)) and not isinstance(total, bool):
                cost += total
    result = {
        "nAssistantMessages": assistant,
        "malformedLines": malformed,
        "costUnknownTurns": unknown,
        "byUpstream": upstreams(turns),
    }
    if assistant == 0:
        result.update({key: None for key in TOKEN_KEYS})
        result["costUsd"] = None
        return result
    result.update(totals)
    result["costUsd"] = None if unknown or cost <= 0 else cost
    return result


def session_extras(sessions_dir):
    """Pier's extra columns from the trial's collected v4 session files.

    peak = the largest single assistant input context (input + cache reads and
    writes); summarizations = compaction entries; steps = assistant messages
    (evals/README.md maps turns to those). All-or-none per E9.
    """
    peak = 0
    compactions = 0
    steps = 0
    root = Path(sessions_dir)
    files = sorted(root.rglob("*.jsonl")) if root.is_dir() else []
    for path in files:
        entries, _ = json_lines(path)
        for entry in entries:
            if entry.get("kind") != "entry":
                continue
            if entry.get("type") == "compaction":
                compactions += 1
                continue
            message = entry.get("message")
            if not isinstance(message, dict) or message.get("role") != "assistant":
                continue
            steps += 1
            context = {key: 0 for key in TOKEN_KEYS}
            _add_tokens(context, message.get("usage"))
            peak = max(
                peak, sum(value for key, value in context.items() if key != "output")
            )
    if steps == 0:
        return {
            "peak_context_tokens": None,
            "summarization_count": None,
            "n_agent_steps": None,
        }
    return {
        "peak_context_tokens": peak,
        "summarization_count": compactions,
        "n_agent_steps": steps,
    }


def config_fingerprint(yi_version, model, mode, suite_at_rev):
    """The eval-ledger's config column: what was run, in twelve hex chars."""
    payload = json.dumps(
        {
            "adapterVersion": ADAPTER_VERSION,
            "model": model,
            "mode": mode,
            "suiteAtRev": suite_at_rev,
            "yiVersion": yi_version,
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:12]
