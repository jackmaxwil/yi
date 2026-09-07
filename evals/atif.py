#!/usr/bin/env python3
"""A v4 session file as an ATIF-v1.7 trajectory (harbor RFC 0001).

    python3 evals/atif.py <session.jsonl> [--agent-version V] [--model M] > trajectory.json

One Step per user message and per assistant message on the main lane; the
tool results that follow an assistant message are its observation. Metrics
follow harbor's own convention (pi.py:260): prompt tokens include cache
reads. Custom entries (todo, intercepts) and compactions carry nothing a
Step names and are skipped; the session file stays the record.
"""

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

SCHEMA = "ATIF-v1.7"


def _iso(ms):
    if not isinstance(ms, (int, float)) or isinstance(ms, bool):
        return None
    return datetime.fromtimestamp(ms / 1000, timezone.utc).isoformat().replace("+00:00", "Z")


def _text(content, kind="text", key="text"):
    if isinstance(content, str):
        return content if kind == "text" else ""
    return "".join(
        block.get(key) or ""
        for block in content or []
        if isinstance(block, dict) and block.get("type") == kind
    )


def _drop_none(value):
    if isinstance(value, dict):
        return {k: _drop_none(v) for k, v in value.items() if v is not None}
    if isinstance(value, list):
        return [_drop_none(v) for v in value]
    return value


def _metrics(usage):
    if not isinstance(usage, dict) or usage.get("unknown") is True:
        return None
    read = int(usage.get("cacheRead") or 0)
    cost = (usage.get("cost") or {}).get("total")
    return {
        "prompt_tokens": int(usage.get("input") or 0) + read,
        "completion_tokens": int(usage.get("output") or 0),
        "cached_tokens": read,
        "cost_usd": float(cost) if isinstance(cost, (int, float)) and not isinstance(cost, bool) else None,
    }


def convert(lines, agent_version="unknown", model=None):
    header, entries = {}, []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if value.get("kind") == "header":
            header = value
        elif value.get("kind") == "entry" and value.get("lane", "main") == "main":
            entries.append(value)
    steps, totals = [], {"prompt": 0, "completion": 0, "cached": 0, "cost": 0.0, "priced": 0}
    models = []
    for entry in entries:
        if entry.get("type") != "message":
            continue
        message = entry.get("message") or {}
        role = message.get("role")
        stamp = _iso(message.get("timestamp") or entry.get("timestamp"))
        if role == "user":
            steps.append({"step_id": len(steps) + 1, "timestamp": stamp, "source": "user",
                          "message": _text(message.get("content"))})
        elif role == "assistant":
            calls = [
                {"tool_call_id": block.get("id"), "function_name": block.get("name"),
                 "arguments": block.get("arguments") if isinstance(block.get("arguments"), dict) else {}}
                for block in message.get("content") or []
                if isinstance(block, dict) and block.get("type") == "toolCall"
            ]
            metrics = _metrics(message.get("usage"))
            if metrics:
                totals["prompt"] += metrics["prompt_tokens"]
                totals["completion"] += metrics["completion_tokens"]
                totals["cached"] += metrics["cached_tokens"]
                if metrics["cost_usd"] is not None:
                    totals["cost"] += metrics["cost_usd"]
                    totals["priced"] += 1
            if message.get("model"):
                models.append(message["model"])
            steps.append({
                "step_id": len(steps) + 1, "timestamp": stamp, "source": "agent",
                "model_name": message.get("model"),
                "message": _text(message.get("content")),
                "reasoning_content": _text(message.get("content"), "thinking", "thinking") or None,
                "tool_calls": calls or None,
                "metrics": metrics,
            })
        elif role == "toolResult" and steps and steps[-1]["source"] == "agent":
            result = {"source_call_id": message.get("toolCallId"), "content": _text(message.get("content"))}
            observation = steps[-1].setdefault("observation", {"results": []})
            observation["results"].append(result)
    assistant_steps = sum(1 for step in steps if step["source"] == "agent")
    trajectory = {
        "schema_version": SCHEMA,
        "session_id": header.get("id"),
        "agent": {"name": "yi", "version": agent_version, "model_name": model or (models[0] if models else None)},
        "steps": steps,
        "final_metrics": {
            "total_prompt_tokens": totals["prompt"],
            "total_completion_tokens": totals["completion"],
            "total_cached_tokens": totals["cached"],
            "total_cost_usd": totals["cost"] if totals["priced"] == assistant_steps and assistant_steps else None,
            "total_steps": len(steps),
        } if assistant_steps else None,
    }
    return _drop_none(trajectory)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("session", type=Path)
    parser.add_argument("--agent-version", default="unknown")
    parser.add_argument("--model")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args(argv)
    try:
        lines = args.session.read_text(errors="replace").splitlines()
    except OSError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    text = json.dumps(convert(lines, args.agent_version, args.model), indent=2) + "\n"
    if args.out:
        args.out.write_text(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
