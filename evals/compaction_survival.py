#!/usr/bin/env python3
"""Identifier and constraint survival across compaction entries in a session JSONL.

Zero spend. Reads v4 session files; reports per-round ratios so a T3 run can
fill docs/eval-ledger.md's ident-survival column.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

IDENT_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_/:.#-")
CONSTRAINT = ("must", "never", "don't", "dont", "always", "only")


def identifiers(text: str) -> set[str]:
    out: set[str] = set()
    buf: list[str] = []
    for ch in text:
        if ch in IDENT_CHARS:
            buf.append(ch)
        else:
            flush(buf, out)
    flush(buf, out)
    return out


def flush(buf: list[str], out: set[str]) -> None:
    if not buf:
        return
    tok = "".join(buf)
    buf.clear()
    if keep(tok):
        out.add(tok)


def keep(tok: str) -> bool:
    if len(tok) < 2:
        return False
    if "://" in tok or "/" in tok:
        return True
    if tok.startswith("E") and tok[1:].isdigit() and len(tok) >= 5:
        return True
    if tok.startswith("#") and tok[1:].isdigit():
        return True
    if "." in tok:
        stem, _, ext = tok.rpartition(".")
        if stem and ext.isalnum():
            return True
    if "_" in tok and any(ch.isalpha() for ch in tok):
        return True
    seen_lower = False
    for ch in tok:
        if ch.islower():
            seen_lower = True
        elif seen_lower and ch.isupper():
            return True
    return False


def message_text(message: dict) -> tuple[str, str]:
    role = message.get("role") or ""
    if role == "user":
        return "user", user_text(message.get("content"))
    if role == "assistant":
        return "assistant", content_text(message.get("content"))
    if role == "bashExecution":
        return "tool", " ".join(
            str(message.get(key) or "")
            for key in ("command", "output", "fullOutputPath")
        )
    if role == "toolResult":
        return "tool", content_text(message.get("content"))
    return "", ""


def user_text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return content_text(content)
    return ""


def content_text(blocks) -> str:
    if not isinstance(blocks, list):
        return ""
    parts = []
    for block in blocks:
        if not isinstance(block, dict):
            continue
        kind = block.get("type")
        if kind == "text":
            parts.append(block.get("text") or "")
        elif kind == "image":
            parts.append(block.get("mimeType") or "")
        elif kind == "toolCall":
            parts.append(block.get("name") or "")
            parts.append(json.dumps(block.get("arguments") or {}, sort_keys=True))
    return " ".join(parts)


def constraint_lines(text: str) -> list[str]:
    lines = []
    for line in text.splitlines() or [text]:
        lower = line.lower()
        if any(word in lower for word in CONSTRAINT):
            lines.append(line.strip())
    return lines


def load_entries(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("kind") == "entry":
            rows.append(row)
    return rows


def measure(path: Path) -> dict:
    entries = load_entries(path)
    rounds = []
    prev = 0
    for index, entry in enumerate(entries):
        if entry.get("type") != "compaction":
            continue
        span = entries[prev:index]
        prev = index + 1
        lanes = {"user": set(), "assistant": set(), "tool": set()}
        constraints: list[str] = []
        for item in span:
            if item.get("type") != "message":
                continue
            message = item.get("message") or {}
            lane, text = message_text(message)
            if lane:
                lanes[lane] |= identifiers(text)
            if lane == "user":
                constraints.extend(constraint_lines(text))
        summary = entry.get("summary") or ""
        details = entry.get("details") or {}
        surviving = identifiers(summary)
        round_row = {
            "id": entry.get("id"),
            "dropped_reported": (details.get("dropped") if isinstance(details, dict) else None),
            "lanes": {},
        }
        for lane, idents in lanes.items():
            total = len(idents)
            kept = len(idents & surviving)
            round_row["lanes"][lane] = {
                "total": total,
                "kept": kept,
                "ratio": (kept / total) if total else 1.0,
            }
        const_total = len(constraints)
        const_kept = sum(1 for line in constraints if line in summary)
        round_row["constraints"] = {
            "total": const_total,
            "kept": const_kept,
            "ratio": (const_kept / const_total) if const_total else 1.0,
        }
        rounds.append(round_row)
    return {"file": str(path), "rounds": rounds}


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: compaction_survival.py <session.jsonl>", file=sys.stderr)
        return 2
    report = measure(Path(argv[1]))
    json.dump(report, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
