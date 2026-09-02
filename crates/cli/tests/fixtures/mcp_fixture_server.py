#!/usr/bin/env python3
"""Deterministic stdio MCP server for yi mcp e2e tests.

Speaks newline-delimited JSON-RPC 2.0: initialize, tools/list, tools/call
(echo + add), resources/list, resources/read, prompts/list, ping.
Exits on stdin EOF.
"""
import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Echoes the message argument back.",
        "inputSchema": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
        },
    },
    {
        "name": "add",
        "description": "Adds two numbers and searches nothing.",
        "inputSchema": {
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"],
        },
    },
]


def result_for(method, params):
    if method == "initialize":
        return {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
            "serverInfo": {"name": "yi-fixture", "version": "1.0.0"},
            "instructions": "Fixture server. The echo tool repeats input.",
        }
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "echo":
            text = arguments.get("message", "")
            return {"content": [{"type": "text", "text": text}], "isError": False}
        if name == "add":
            total = arguments.get("a", 0) + arguments.get("b", 0)
            return {"content": [{"type": "text", "text": str(total)}], "isError": False}
        return {"content": [{"type": "text", "text": f"no such tool {name}"}], "isError": True}
    if method == "resources/list":
        return {"resources": [{"uri": "note://alpha", "name": "alpha"}]}
    if method == "resources/read":
        uri = params.get("uri", "")
        return {"contents": [{"uri": uri, "mimeType": "text/plain", "text": "alpha"}]}
    if method == "prompts/list":
        return {"prompts": []}
    if method == "ping":
        return {}
    return None


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        if "id" not in message:
            continue  # notification
        result = result_for(message.get("method"), message.get("params") or {})
        if result is None:
            reply = {
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {"code": -32601, "message": f"method not found: {message.get('method')}"},
            }
        else:
            reply = {"jsonrpc": "2.0", "id": message["id"], "result": result}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
