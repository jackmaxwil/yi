#!/usr/bin/env python3
"""Session JSONL -> behavior cassette (J3, plan §3 micro-tier).

The v4 session file already holds everything a cassette needs: the assistant
entry IS the provider response and the toolResult entry IS the tool result, so
recording is a post-hoc read of a file Yi already wrote. No CLI flag, no
capture layer, no runtime change.

    python3 evals/record.py <session.jsonl> --id <cassette-id> \\
        --description <text> [--out crates/runtime/tests/fixtures/behavior/x.json]

Unrepresentable input is fatal (exit 2) with the reason named. A silent skip
would emit a cassette that replays a session which never happened.
"""

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT.parent / "skills" / "yi" / "session-mining"))

import extract  # noqa: E402

# Entry types that cannot change what a cassette replays: the recorded
# responses already carry whatever a model or tool-set switch did to them.
IGNORED_TYPES = frozenset(
    {"custom", "model_change", "thinking_level_change", "active_tools_change"}
)


class Unrepresentable(Exception):
    """Input the cassette schema cannot carry without inventing something."""


def scrub(text):
    """§10 redaction, borrowed whole from the mining extractor so the two
    artifacts mask by one vocabulary."""
    cleaned = extract.redact(text)
    if text.endswith("\n") and not cleaned.endswith("\n"):
        cleaned += "\n"
    return cleaned


def scrub_values(value):
    if isinstance(value, dict):
        return {key: scrub_values(item) for key, item in value.items()}
    if isinstance(value, list):
        return [scrub_values(item) for item in value]
    if isinstance(value, str):
        return scrub(value)
    return value


def text_blocks(content, where):
    if isinstance(content, str):
        return content
    parts = []
    for block in content or []:
        if not isinstance(block, dict) or block.get("type") != "text":
            kind = block.get("type") if isinstance(block, dict) else type(block).__name__
            raise Unrepresentable(f"{where} content block `{kind}` has no cassette shape")
        parts.append(block.get("text") or "")
    return "\n".join(parts)


def response_spec(message, notes):
    text_parts, calls = [], []
    for block in message.get("content") or []:
        kind = block.get("type") if isinstance(block, dict) else None
        if kind == "text":
            text_parts.append(block.get("text") or "")
        elif kind == "toolCall":
            calls.append(
                {
                    "id": block.get("id") or "",
                    "name": block.get("name") or "",
                    "arguments": scrub_values(block.get("arguments") or {}),
                }
            )
        elif kind == "thinking":
            notes.append("dropped a thinking block: the cassette schema has no slot for it")
        else:
            raise Unrepresentable(f"assistant content block `{kind}` has no cassette shape")
    spec = {}
    text = scrub("\n".join(text_parts))
    if text or not calls:
        spec["text"] = text
    if calls:
        spec["toolCalls"] = calls
    usage = message.get("usage") or {}
    total = int(usage.get("totalTokens") or 0)
    if total > 0:
        spec["usageTotal"] = total
        spec["usageInput"] = int(usage.get("input") or 0)
    return spec


def load_entries(path):
    entries = []
    for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except ValueError as error:
            raise Unrepresentable(f"line {number} is not JSON: {error}") from error
        if not isinstance(obj, dict):
            raise Unrepresentable(f"line {number} is not a JSON object")
        if obj.get("kind") == "entry":
            entries.append(obj)
    entries.sort(key=lambda entry: entry.get("seq") or 0)
    return entries


def convert(entries, notes):
    turns, stubs = [], {}
    for entry in entries:
        lane = entry.get("lane")
        if lane != "main":
            raise Unrepresentable(f"entry {entry.get('id')} is on lane `{lane}`, not main")
        kind = entry.get("type")
        if kind in IGNORED_TYPES:
            notes.append(f"skipped a `{kind}` entry: it does not shape the replay")
            continue
        if kind != "message":
            raise Unrepresentable(
                f"entry {entry.get('id')} of type `{kind}` rewrites the context a linear"
                " cassette replays"
            )
        message = entry.get("message") or {}
        role = message.get("role")
        if role == "user":
            turns.append({"user": scrub(text_blocks(message.get("content"), "user"))})
        elif role == "assistant":
            if not turns:
                raise Unrepresentable(
                    f"assistant entry {entry.get('id')} precedes every user message"
                )
            turns[-1].setdefault("responses", []).append(response_spec(message, notes))
        elif role == "toolResult":
            name = message.get("toolName") or ""
            stubs.setdefault(name, []).append(
                {
                    "text": scrub(text_blocks(message.get("content"), "toolResult")),
                    "isError": bool(message.get("isError")),
                }
            )
        else:
            raise Unrepresentable(
                f"entry {entry.get('id')} carries role `{role}`, which the cassette schema lacks"
            )
    for turn in turns:
        turn.setdefault("responses", [])
    return turns, stubs


def record(path, cassette_id, description):
    """Return (cassette, notes). Raises [`Unrepresentable`] rather than
    guessing at input the schema cannot carry."""
    notes = []
    turns, stubs = convert(load_entries(path), notes)
    if not turns:
        raise Unrepresentable(f"{path.name} holds no main-lane user message")
    cassette = {
        "id": cassette_id,
        "description": description,
        "provenance": {"kind": "recorded", "session": path.name},
    }
    if stubs:
        cassette["stubs"] = [
            {"name": name, "results": results} for name, results in stubs.items()
        ]
    cassette["turns"] = turns
    cassette["assertions"] = []
    return cassette, notes


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("session", type=Path, help="a v4 session JSONL file")
    parser.add_argument("--id", required=True, help="cassette id; must equal the filename stem")
    parser.add_argument("--description", required=True, help="what the case defends")
    parser.add_argument("--out", type=Path, help="write here instead of stdout")
    args = parser.parse_args(argv)
    try:
        cassette, notes = record(args.session, args.id, args.description)
    except Unrepresentable as error:
        print(f"cannot record {args.session}: {error}", file=sys.stderr)
        return 2
    for note in notes:
        print(note, file=sys.stderr)
    text = json.dumps(cassette, indent=2) + "\n"
    if args.out:
        args.out.write_text(text)
        print(f"wrote {args.out}; add the pass condition to `assertions` before committing",
              file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
