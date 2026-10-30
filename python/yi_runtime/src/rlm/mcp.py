"""MCP access for kernel code: a subprocess wrapper over `yi mcp --json`.

Yi's design (YI_DESIGN.md §7.6) keeps MCP client state out of the kernel:
a connect asks the host, which runs `yi mcp connect` from its own
`~/.yi/mcp.json` and writes the session store this process cannot; every
other call shells out to the one-shot `yi mcp` CLI, which reads that store.
No sockets or SDK live in this process.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
from typing import Any


__all__ = ["McpStartupError", "McpToolError", "call_tool", "list_tools"]

_DEFAULT_STARTUP_TIMEOUT = 20.0
_DEFAULT_CALL_TIMEOUT = 60.0


class McpToolError(RuntimeError):
    """Raised when an MCP tool call returns a result flagged as an error."""


def _parse_result(result: Any) -> Any:
    if not isinstance(result, dict):
        return result
    content = result.get("content")
    texts = [
        item.get("text")
        for item in content or []
        if isinstance(item, dict) and item.get("type") == "text" and isinstance(item.get("text"), str)
    ]
    if result.get("isError"):
        raise McpToolError("\n".join(texts) or "MCP tool call failed")
    if len(texts) == 1:
        try:
            return json.loads(texts[0])
        except ValueError:
            return texts[0]
    if texts and content is not None and len(texts) == len(content):
        return texts
    return result


class McpStartupError(RuntimeError):
    """The `yi mcp` CLI failed while connecting a server."""


def _yi_bin() -> str:
    return os.environ.get("YI_BIN") or "yi"


def _session(server: str) -> str:
    return "krnl-" + re.sub(r"[^A-Za-z0-9_-]", "-", server)


def _validate_name(value: str, label: str) -> None:
    if not isinstance(value, str) or not value:
        raise TypeError(f"{label} must be a non-empty string")


async def _run(args: list[str], *, stdin: bytes | None = None, timeout: float) -> Any:
    proc = await asyncio.create_subprocess_exec(
        _yi_bin(),
        "mcp",
        *args,
        stdin=asyncio.subprocess.PIPE if stdin is not None else asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        out, err = await asyncio.wait_for(proc.communicate(stdin), timeout)
    except TimeoutError:
        proc.kill()
        await proc.wait()
        raise
    if proc.returncode != 0:
        message = err.decode("utf-8", errors="replace").strip() or f"yi mcp exited {proc.returncode}"
        raise RuntimeError(message)
    text = out.decode("utf-8", errors="replace").strip()
    return json.loads(text) if text else None


async def _connect(server: str) -> Any:
    from . import host_request

    try:
        return await host_request("mcp.connect", {"server": server, "session": _session(server)})
    except RuntimeError as exc:
        raise McpStartupError(f"MCP server '{server}' failed to connect: {exc}") from exc


async def list_tools(server: str) -> list[dict[str, Any]]:
    _validate_name(server, "server")
    payload = await _connect(server)
    connected = payload.get("server") if isinstance(payload, dict) else None
    tools = connected.get("tools") if isinstance(connected, dict) else None
    return tools if isinstance(tools, list) else []


async def call_tool(server: str, tool: str, arguments: dict[str, Any] | None = None) -> Any:
    _validate_name(server, "server")
    _validate_name(tool, "tool")
    if arguments is not None and not isinstance(arguments, dict):
        raise TypeError("arguments must be a dict or None")
    args = [f"@{_session(server)}", "tools-call", tool, "--json"]
    stdin = json.dumps(arguments or {}).encode()
    try:
        result = await _run(args, stdin=stdin, timeout=_DEFAULT_CALL_TIMEOUT)
    except RuntimeError as exc:
        if "unknown session" not in str(exc):
            raise
        await _connect(server)
        result = await _run(args, stdin=stdin, timeout=_DEFAULT_CALL_TIMEOUT)
    return _parse_result(result)
