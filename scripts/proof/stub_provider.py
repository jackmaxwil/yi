#!/usr/bin/env python3
"""OpenAI chat-completions SSE stub: streams one reply file at a provider-like cadence.

usage: stub_provider.py PORT REPLY_FILE TOKENS_PER_SECOND
"""
import http.server
import json
import random
import sys
import time

TEXT = open(sys.argv[2]).read()
RATE = float(sys.argv[3])


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        print(f"{time.time():.1f} {fmt % args}", file=sys.stderr, flush=True)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def send(payload):
            body = payload if isinstance(payload, str) else json.dumps(payload)
            data = f"data: {body}\n\n".encode()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
            self.wfile.flush()

        def chunk(delta, finish=None, **extra):
            return {"id": "x", "object": "chat.completion.chunk", "created": 0, "model": "local-slow",
                    "choices": [{"index": 0, "delta": delta, "finish_reason": finish}], **extra}

        rng, at = random.Random(7), 0
        while at < len(TEXT):
            # A network burst of 2 to 9 tokens (about 4 characters each), then the wait they cost.
            tokens = rng.randint(2, 9)
            piece = TEXT[at:at + tokens * 4]
            at += len(piece)
            send(chunk({"content": piece}))
            time.sleep(tokens / RATE)
        send(chunk({}, "stop", usage={"prompt_tokens": 10, "completion_tokens": 100, "total_tokens": 110}))
        send("[DONE]")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
