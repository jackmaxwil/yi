"""Roles: who executes a delegated todo, and the wall it runs behind."""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any


def _pruned(value: dict) -> dict:
    return {key: item for key, item in value.items() if item not in (None, [], {}, ())}


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
