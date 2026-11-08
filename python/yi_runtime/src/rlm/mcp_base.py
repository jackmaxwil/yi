"""MCP integration base for kernel code, over the `yi mcp` subprocess wrapper.

An integration subclasses :class:`McpIntegration`, declares the MCP ``server``
it targets, and is imported in the kernel like any other skill. Tool calls are
bound as async methods, so the agent writes ordinary Python:

    import linear
    issues = await linear.list_issues(team="Engineering")

Credentials live host-side (`yi mcp login`); this process never sees tokens.
"""

from __future__ import annotations

from typing import Any

from .mcp import McpToolError

__all__ = ["McpIntegration", "McpToolError", "NotEnabled"]


class NotEnabled(RuntimeError):
    """Raised when an integration has no usable credentials.

    The integration is installed but not logged in. The message tells the agent
    how to enable it so it can relay that to the user rather than retrying.
    """

    def __init__(self, server: str):
        self.server = server
        super().__init__(
            f"The '{server}' integration is not enabled: no credentials found. "
            f"Tell the user to run `yi mcp login {server}` to connect it. "
            f"Do not ask them to set environment variables."
        )


class McpIntegration:
    """Binds an MCP server's tools as async methods named after each tool."""

    server: str | None = None

    def _server(self) -> str:
        server = self.server or type(self).__name__.lower()
        if not isinstance(server, str) or not server:
            raise ValueError("McpIntegration requires a server name")
        return server

    async def list_tools(self) -> list[dict[str, Any]]:
        from . import mcp

        try:
            return await mcp.list_tools(self._server())
        except RuntimeError as exc:
            if "login" in str(exc):
                raise NotEnabled(self._server()) from exc
            raise

    def __getattr__(self, name: str):
        if name.startswith("_"):
            raise AttributeError(name)

        async def call(**kwargs: Any) -> Any:
            from . import mcp

            try:
                return await mcp.call_tool(self._server(), name, kwargs)
            except RuntimeError as exc:
                if "login" in str(exc):
                    raise NotEnabled(self._server()) from exc
                raise

        call.__name__ = name
        return call
