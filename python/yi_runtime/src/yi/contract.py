"""Contract builders: the item list the host verifies a todo against at ``done``.

A builder names its criterion by content, never by path: a ``local://`` input is
read once, when the todo is declared, and frozen into the plan's artifact store,
so nothing a child edits later can move the bar it is judged by.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import Any

MANIFEST_FORMAT = 1
JSON_TYPE = "application/json"


def canonical(value: Any) -> str:
    """The one JSON spelling Rust and Python hash alike (sorted keys, no spaces)."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def freeze(text: str, media_type: str, provenance: str | None = None) -> tuple[dict, dict]:
    """An artifact reference for ``text`` and the blob that rides the op to the store."""
    data = text.encode("utf-8")
    ref: dict[str, Any] = {
        "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
        "media_type": media_type,
        "length": len(data),
    }
    if provenance:
        ref["provenance"] = provenance
    return ref, {"media_type": media_type, "text": text}


def _manifest(command: str, timeout_ms: int, cwd: str | None, protected: Any, env: Any) -> str:
    if not isinstance(command, str) or not command.strip():
        raise ValueError("a checker needs one shell command")
    return canonical(
        {
            "manifest": MANIFEST_FORMAT,
            "command": command,
            "cwd": "snapshot_subdir" if cwd else "snapshot_root",
            "cwd_subdir": cwd or None,
            "protected": list(protected),
            "timeout_ms": timeout_ms,
            "env": list(env),
            "reads_outside_snapshot": False,
        }
    )


@dataclass(frozen=True)
class Item:
    """One contract line; build it with ``cmd``, ``schema`` or ``example``."""

    kind: str
    sources: tuple
    critical: bool
    weight: int
    timeout_ms: int | None = None
    id: str | None = None


def cmd(
    command: str,
    *,
    critical: bool = False,
    weight: int = 1,
    timeout: float = 60.0,
    cwd: str | None = None,
    protected: tuple | list = (),
    env: tuple | list = (),
    id: str | None = None,
) -> Item:
    """A shell command that must exit 0 in a snapshot of the workspace.

    ``cwd`` is a directory under the snapshot root, ``protected`` the paths the
    check may not change, ``env`` the variable names it may see (never values).

        check = cmd("pytest -q tests/", critical=True)
    """
    timeout_ms = int(timeout * 1000)
    text = _manifest(command, timeout_ms, cwd, protected, env)
    return Item("cmd", ((text, JSON_TYPE),), critical, weight, timeout_ms, id)


def schema(source: dict | str, *, critical: bool = False, weight: int = 1, id: str | None = None) -> Item:
    """The todo's output must satisfy a JSON schema: a dict, or a ``local://`` file.

        shape = schema({"type": "object", "required": ["passed"]}, critical=True)
    """
    return Item("schema", (_source(source),), critical, weight, None, id)


def example(
    cases: list | str,
    runner: str,
    *,
    critical: bool = False,
    weight: int = 1,
    timeout: float = 60.0,
    id: str | None = None,
) -> Item:
    """Each case's ``input`` goes to ``runner`` on stdin and must print its ``expected``.

        cases = example([{"input": 2, "expected": 4}], "python double.py", critical=True)
    """
    timeout_ms = int(timeout * 1000)
    manifest = _manifest(runner, timeout_ms, None, (), ())
    return Item("example", (_source(cases), (manifest, JSON_TYPE)), critical, weight, timeout_ms, id)


def _source(source: Any) -> Any:
    if isinstance(source, str):
        if not source.startswith("local://"):
            raise ValueError(f"{source!r} is not a local:// url; pass the value itself instead")
        return source
    return (canonical(source), JSON_TYPE)


@dataclass(frozen=True)
class Contract:
    """What ``contract(...)`` returns; ``plan.todo(accept=...)`` renders it."""

    items: tuple
    threshold: int = 1000
    min_coverage: int = 1000

    async def render(self, contract_class: str, fetch: Any) -> tuple[dict, list[dict]]:
        """The wire contract and the blobs it names; ``fetch`` reads a ``local://`` url."""
        blobs: list[dict] = []
        items = []
        for index, item in enumerate(self.items, 1):
            refs = []
            for source in item.sources:
                if isinstance(source, str):
                    ref, blob = freeze(await fetch(source), JSON_TYPE, provenance=source)
                else:
                    ref, blob = freeze(*source)
                refs.append(ref)
                blobs.append(blob)
            if item.kind == "cmd":
                decider = {"cmd": {"checker": refs[0], "timeout_ms": item.timeout_ms}}
            elif item.kind == "schema":
                decider = {"schema": {"schema": refs[0]}}
            else:
                decider = {"example": {"cases": refs[0], "runner": refs[1], "timeout_ms": item.timeout_ms}}
            items.append(
                {
                    "id": item.id or f"{item.kind}{index}",
                    "critical": item.critical,
                    "weight": item.weight,
                    "decider": decider,
                }
            )
        wire = {
            "class": contract_class,
            "items": items,
            "threshold": self.threshold,
            "min_coverage": self.min_coverage,
        }
        return wire, blobs


def contract(*items: Item, threshold: int = 1000, min_coverage: int = 1000) -> Contract:
    """Group items into the contract a todo is verified against at ``done``.

    ``threshold`` and ``min_coverage`` are permille: 1000 means every item
    decides and every decided item passes. The host, not this library, runs
    the items and aggregates the verdict.

        accept = contract(cmd("make -s check", critical=True))
    """
    if not items or not all(isinstance(item, Item) for item in items):
        raise TypeError("contract takes one or more items built by cmd, schema or example")
    return Contract(tuple(items), threshold, min_coverage)
