"""MCP access for kernel code: a subprocess wrapper over `yi mcp --json`.

Yi's design (YI_DESIGN.md §5.2) keeps MCP client state out of the kernel:
every call shells out to the one-shot `yi mcp` CLI, which owns config,
sessions, and auth. No sockets or SDK live in this process.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
from typing import Any

from .mcp_base import McpToolError, _parse_result

__all__ = ["McpStartupError", "call_tool", "close", "list_tools", "reload"]

_DEFAULT_STARTUP_TIMEOUT = 20.0
_DEFAULT_CALL_TIMEOUT = 60.0


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
    try:
        return await _run(
            ["connect", server, f"@{_session(server)}", "--json"],
            timeout=_DEFAULT_STARTUP_TIMEOUT,
        )
    except RuntimeError as exc:
        raise McpStartupError(f"MCP server '{server}' failed to connect: {exc}") from exc


async def list_tools(server: str) -> list[dict[str, Any]]:
    _validate_name(server, "server")
    payload = await _connect(server)
    tools = payload.get("tools") if isinstance(payload, dict) else None
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


async def reload(server: str | None = None) -> None:
    if server is not None:
        _validate_name(server, "server")
        await _run([f"@{_session(server)}", "restart", "--json"], timeout=_DEFAULT_STARTUP_TIMEOUT)


async def close() -> None:
    return None
