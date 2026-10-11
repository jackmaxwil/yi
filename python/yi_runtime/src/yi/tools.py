"""yi.tools: this session's own tools, called from a cell.

Each call is judged as the model's own call is, by the user's rules, the wall and the
permission check, and the cell's ``ipython`` result lists it, so an edit made here is an edit
compaction and the todo list see. A call returns a ``Result`` (``text`` is the view the model
would have read, ``details`` the tool's own fields) and raises on failure: ``UnknownTool``,
``InvalidInput``, ``Refused`` (a gate said no) or ``ToolFailure`` (its ``kind`` is
``cancelled`` for a call made after the cell that started it ended).

``search("edit")`` finds a tool, ``help(yi.tools.grep)`` shows its parameters, and
``excluded()`` names the tools a cell cannot call and why. An edit cites the ``[path#TAG]``
headers a read printed:

    hits = await yi.tools.grep(pattern="select_parts", path="crates")
    view = await yi.tools.read(path="crates/runtime/src/select.rs")
"""
from __future__ import annotations

import inspect
import keyword
from typing import Any

import rlm

from .plan import Refused

__all__ = ["Result", "ToolError", "UnknownTool", "InvalidInput", "ToolFailure", "Refused",
           "call", "search", "excluded"]

SEARCH_LIMIT = 10
_TYPES = {"string": str, "integer": int, "number": float, "boolean": bool, "array": list,
          "object": dict}
_catalog: dict[str, Any] | None = None


class ToolError(RuntimeError):
    """A ``yi.tools`` call the host would not run or that failed; ``kind`` says which."""

    def __init__(self, message: str, kind: str, suggestions: list[str] | None = None,
                 result: "Result | None" = None) -> None:
        super().__init__(message)
        self.kind = kind
        self.suggestions = suggestions or []
        self.result = result


class UnknownTool(ToolError, AttributeError):
    """No such tool in this session, or one a cell cannot call; ``suggestions`` lists the names."""


class InvalidInput(ToolError, ValueError):
    """The tool refused these arguments before running."""


class ToolFailure(ToolError):
    """The tool ran and failed; ``result`` holds what it returned."""


class Result:
    """One call's outcome: ``text`` is what the model would have read, ``details`` the tool's fields."""

    def __init__(self, tool: str, text: str, details: dict[str, Any]) -> None:
        self.tool = tool
        self.text = text
        self.details = details

    def __str__(self) -> str:
        return self.text

    def __repr__(self) -> str:
        return self.text


async def call(name: str, **args: Any) -> Result:
    """Call tool ``name`` with ``args``; an argument passed as ``None`` is not sent."""
    sent = {key: value for key, value in args.items() if value is not None}
    reply = await rlm.host_request("tool.call", {"name": name, "args": sent})
    result = Result(name, reply.get("text", ""), reply.get("details") or {})
    if reply.get("ok"):
        return result
    kind = reply.get("kind") or "tool_failure"
    message = reply.get("message") or result.text or f"{name} failed"
    if kind in ("unknown_tool", "excluded_tool"):
        raise UnknownTool(message, kind, reply.get("suggestions"))
    if kind == "invalid_input":
        raise InvalidInput(message, kind)
    if kind == "refused":
        raise Refused(message, {"code": "refused", "kind": "refused", "tool": name})
    raise ToolFailure(message, kind, result=result)


def _load() -> dict[str, Any]:
    global _catalog
    if _catalog is None:
        _catalog = rlm._host_request_blocking("tool.list", {})
    return _catalog


def _tools() -> dict[str, dict[str, Any]]:
    return {tool["name"]: tool for tool in _load().get("tools", [])}


def excluded() -> list[dict[str, str]]:
    """The tools this session holds that a cell cannot call, each with why."""
    return list(_load().get("excluded", []))


def search(query: str = "", offset: int = 0, limit: int = SEARCH_LIMIT) -> dict[str, Any]:
    """Tools whose name or description holds every word of ``query``, name matches first.

    The result names how many matched, how many it shows, and the call that shows the rest.
    """
    words = query.lower().split()
    scored = []
    for name, tool in _tools().items():
        text = f"{name} {tool.get('description', '')}".lower()
        if all(word in text for word in words):
            scored.append((-sum(word in name for word in words), name, tool))
    scored.sort(key=lambda row: (row[0], row[1]))
    shown = scored[offset:offset + limit]
    rows = [{"name": name, "signature": f"yi.tools.{name}{_signature(tool)}",
             "description": tool.get("description", "")} for _, name, tool in shown]
    end = offset + len(shown)
    page = {"tools": rows, "matched": len(scored), "shown": len(shown)}
    if end < len(scored):
        page["more"] = (f"[{len(shown)} of {len(scored)} shown at limit={limit}; the rest: "
                        f"yi.tools.search({query!r}, offset={end})]")
    left_out = [row["name"] for row in excluded()]
    if left_out:
        page["left_out"] = f"[not callable from a cell: {', '.join(left_out)}; why: yi.tools.excluded()]"
    return page


def _signature(tool: dict[str, Any]) -> inspect.Signature:
    schema = tool.get("parameters") or {}
    required = set(schema.get("required") or [])
    params = []
    for key, prop in (schema.get("properties") or {}).items():
        if not key.isidentifier() or keyword.iskeyword(key):
            continue
        kind = (prop or {}).get("type")
        annotation = _TYPES.get(kind, inspect.Parameter.empty) if isinstance(kind, str) else inspect.Parameter.empty
        default = inspect.Parameter.empty if key in required else None
        params.append(inspect.Parameter(key, inspect.Parameter.KEYWORD_ONLY, default=default,
                                        annotation=annotation))
    params.sort(key=lambda param: param.default is not inspect.Parameter.empty)
    return inspect.Signature(params)


def _bound(name: str, tool: dict[str, Any]):
    async def bound(**args: Any) -> Result:
        return await call(name, **args)

    bound.__name__ = bound.__qualname__ = name
    bound.__doc__ = tool.get("description", "")
    bound.__signature__ = _signature(tool)  # type: ignore[attr-defined]
    return bound


def __getattr__(name: str):
    if name.startswith("__"):
        raise AttributeError(name)
    tools = _tools()
    if name not in tools:
        names = sorted(tools)
        why = {row["name"]: row["why"] for row in excluded()}.get(name)
        message = (f"{name} is not callable from a cell: {why}" if why
                   else f"this session has no tool {name}; it has: {', '.join(names)}")
        raise UnknownTool(message, "excluded_tool" if why else "unknown_tool", names)
    return _bound(name, tools[name])


def __dir__() -> list[str]:
    return sorted(set(__all__) | set(_tools()))
