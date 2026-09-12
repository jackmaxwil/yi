"""Yi memory skill: notes from earlier sessions, written by the host.

One markdown file per fact and `MEMORY.md` as the index; the index loads at
session start. These functions wrap `rlm.host_request` and only work inside
the Yi IPython kernel. Replies never carry a filesystem path.
"""

from __future__ import annotations

from typing import Any

from rlm import host_request


async def save(markdown: str, **fields: str) -> dict[str, Any]:
    """Write one note; the same name updates it.

    `markdown` is the whole file: YAML frontmatter with `name`, `description`
    (the situation, then the rule, one line), and `type` (user, feedback,
    project, or reference), then the fact, **Why:**, and **How to apply:**.
    Keyword fields overlay the frontmatter, so a bare body works too:
    `save(body, type="feedback")`. `scope="global"` keeps the note for every
    repository; the default is this one. Returns name, description, type,
    scope, updated, and warnings for keys that were kept but likely mistyped.
    """
    if not isinstance(markdown, str):
        raise TypeError(f"markdown must be str, got {type(markdown).__name__}")
    return await host_request("memory.save", {"markdown": markdown, "fields": fields})


async def read(name: str, scope: str | None = None) -> dict[str, Any]:
    """Return a note's body by its name or its hook; repo first, then global."""
    payload: dict[str, Any] = {"name": name}
    if scope is not None:
        payload["scope"] = scope
    return await host_request("memory.read", payload)


async def forget(name: str, scope: str | None = None) -> dict[str, Any]:
    """Delete a wrong note and its index line."""
    payload: dict[str, Any] = {"name": name}
    if scope is not None:
        payload["scope"] = scope
    return await host_request("memory.forget", payload)
