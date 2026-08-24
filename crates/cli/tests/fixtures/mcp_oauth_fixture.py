#!/usr/bin/env python3
"""OAuth + streamable-HTTP MCP fixture for yi mcp e2e tests.

Serves on 127.0.0.1:<ephemeral>; prints "PORT <n>" on stdout when ready.
Tokens: authorization_code -> tok-1/ref-1; refresh ref-1 -> tok-2.
"""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

VALID_TOKENS = {"tok-1", "tok-2"}
STATE = {"origin": ""}

TOOLS = [
    {
        "name": "echo",
        "description": "Echoes the message argument back over HTTP.",
        "inputSchema": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
        },
    }
]


def rpc_result(method, params):
    if method == "initialize":
        return {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "yi-oauth-fixture", "version": "1.0.0"},
            "instructions": "HTTP fixture behind OAuth.",
        }
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        args = params.get("arguments") or {}
        return {
            "content": [{"type": "text", "text": args.get("message", "")}],
            "isError": False,
        }
    if method == "ping":
        return {}
    return None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _json(self, code, payload, headers=None):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _unauthorized(self):
        self.send_response(401)
        self.send_header(
            "www-authenticate",
            f'Bearer resource_metadata="{STATE["origin"]}/.well-known/oauth-protected-resource"',
        )
        self.send_header("content-length", "0")
        self.end_headers()

    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path == "/.well-known/oauth-protected-resource":
            return self._json(
                200,
                {
                    "resource": f"{STATE['origin']}/mcp",
                    "authorization_servers": [STATE["origin"]],
                },
            )
        if parsed.path == "/.well-known/oauth-authorization-server":
            return self._json(
                200,
                {
                    "issuer": STATE["origin"],
                    "authorization_endpoint": f"{STATE['origin']}/authorize",
                    "token_endpoint": f"{STATE['origin']}/token",
                    "registration_endpoint": f"{STATE['origin']}/register",
                    "authorization_response_iss_parameter_supported": True,
                },
            )
        if parsed.path == "/authorize":
            query = parse_qs(parsed.query)
            redirect = query["redirect_uri"][0]
            state = query["state"][0]
            location = f"{redirect}?code=code-1&state={state}&iss={STATE['origin']}"
            self.send_response(302)
            self.send_header("location", location)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        if parsed.path == "/mcp":
            return self._unauthorized()
        self.send_response(404)
        self.send_header("content-length", "0")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length).decode() if length else ""
        if self.path == "/register":
            return self._json(201, {"client_id": "client-abc"})
        if self.path == "/token":
            form = parse_qs(body)
            grant = form.get("grant_type", [""])[0]
            if grant == "authorization_code" and form.get("code", [""])[0] == "code-1":
                return self._json(
                    200,
                    {
                        "access_token": "tok-1",
                        "refresh_token": "ref-1",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                    },
                )
            if grant == "refresh_token" and form.get("refresh_token", [""])[0] == "ref-1":
                return self._json(
                    200,
                    {
                        "access_token": "tok-2",
                        "refresh_token": "ref-2",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                    },
                )
            return self._json(400, {"error": "invalid_grant"})
        if self.path == "/mcp":
            auth = self.headers.get("authorization", "")
            if not (auth.startswith("Bearer ") and auth[7:] in VALID_TOKENS):
                return self._unauthorized()
            message = json.loads(body)
            if "id" not in message:
                self.send_response(202)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            result = rpc_result(message.get("method"), message.get("params") or {})
            if result is None:
                return self._json(
                    200,
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "error": {"code": -32601, "message": "method not found"},
                    },
                )
            return self._json(
                200, {"jsonrpc": "2.0", "id": message["id"], "result": result}
            )
        self.send_response(404)
        self.send_header("content-length", "0")
        self.end_headers()


def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    STATE["origin"] = f"http://127.0.0.1:{server.server_port}"
    sys.stdout.write(f"PORT {server.server_port}\n")
    sys.stdout.flush()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    sys.stdin.read()  # exit when the parent closes stdin


if __name__ == "__main__":
    main()
