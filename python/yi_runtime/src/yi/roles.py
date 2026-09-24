"""Roles: who executes a delegated todo, and the wall it runs behind."""
from __future__ import annotations

import hashlib
from dataclasses import dataclass
from typing import Any

import rlm


def _pruned(value: dict) -> dict:
    return {key: item for key, item in value.items() if item not in (None, [], {}, ())}


def _inside(url: str, root: str) -> bool:
    """True when ``url`` is ``root`` itself or an entry under it; a partition is a prefix."""
    return url == root or url.startswith(root.rstrip("/") + "/")


@dataclass(frozen=True)
class Role:
    """A spawn spec plus a wall; build one with ``Writer`` or ``Reader``."""

    role: str
    contract_class: str
    isolation: str | None = None
    model: str | None = None
    effort: str | None = None
    tools: tuple = ()
    deny_write: tuple = ()
    deny_read: tuple = ()
    deny_url: tuple = ()
    context: tuple = ()
    note: str | None = None

    def delegation(self) -> dict[str, Any]:
        """The wire delegation, spelled the way the host writes it back."""
        wall = _pruned(
            {
                "deny_write": list(self.deny_write),
                "deny_read": list(self.deny_read),
                "deny_url": list(self.deny_url),
            }
        )
        spec = _pruned(
            {
                "role": self.role,
                "model": self.model,
                "effort": self.effort,
                "tools": list(self.tools),
                "isolation": self.isolation,
                "wall": wall,
            }
        )
        return _pruned(
            {
                "spec": spec,
                "accept": {"stated": "the todo's contract decides"},
                "context": list(self.context),
                "note": self.note,
            }
        )


def Writer(
    *,
    isolation: str | None = "worktree",
    deny_write: tuple | list = (),
    model: str | None = None,
    effort: str | None = None,
    tools: tuple | list = (),
    context: tuple | list = (),
    note: str | None = None,
) -> Role:
    """A child that changes files, in its own worktree unless told otherwise.

    Its contract needs a critical ``cmd`` or ``example`` item: a writer is
    judged by behavior.

        delegate = Writer(deny_write=["docs/"])
    """
    return Role(
        "writer",
        "writer",
        isolation=isolation,
        model=model,
        effort=effort,
        tools=tuple(tools),
        deny_write=tuple(deny_write),
        context=tuple(context),
        note=note,
    )


def Reader(
    *,
    partition: tuple | list = (),
    deny_read: tuple | list = (),
    model: str | None = None,
    effort: str | None = None,
    tools: tuple | list = (),
    note: str | None = None,
) -> Role:
    """A child that reads and answers; it may write nothing.

    ``partition`` is the context it is bound to. Its contract needs a critical
    ``schema`` item: a reader is judged by the shape of its answer.

        delegate = Reader(partition=["local://docs/api.md"])
    """
    return Role(
        "reader",
        "reader",
        model=model,
        effort=effort,
        tools=tuple(tools),
        deny_write=(".",),
        deny_read=tuple(deny_read),
        context=tuple(partition),
        note=note,
    )


async def verify_quotes(quotes: Any, within: Any = ()) -> list[dict[str, Any]]:
    """Keep the quotes whose ``text`` is on line ``line`` of ``url``, each with the digest it was read at.

    Each url is fetched once through ``rlm.fetch``; a quote that is malformed,
    cites a page that cannot be fetched, or is not on its line is dropped. Give
    ``within`` a reader's partition and a quote citing anything else is dropped
    unread, since the wall that bound the reader is cooperative and the owner
    fetches with the owner's own reach. A kept quote proves provenance, not that
    it supports the answer.

        kept = await verify_quotes([{"url": "local://docs/api.md", "line": 12, "text": "rotate(size)"}],
                                   within=["local://docs"])
    """
    pages: dict[str, str | None] = {}
    kept = []
    for quote in quotes if isinstance(quotes, list) else []:
        try:
            url, line, text = quote["url"], int(quote["line"]), quote["text"].strip()
        except (AttributeError, KeyError, TypeError, ValueError):
            continue
        if within and not any(_inside(url, root) for root in within):
            continue
        if url not in pages:
            try:
                pages[url] = await rlm.fetch(url, as_text=True)
            except (RuntimeError, TypeError):
                pages[url] = None
        page = pages[url]
        lines = (page or "").splitlines()
        if text and 0 < line <= len(lines) and text in lines[line - 1]:
            kept.append({**quote, "digest": "sha256:" + hashlib.sha256(page.encode("utf-8")).hexdigest()})
    return kept
