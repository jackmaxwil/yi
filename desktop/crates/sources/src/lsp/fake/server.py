#!/usr/bin/env python3
"""A scripted language server for the lsp source's tests: replays the exchange in a JSON file.

The script maps a method to a list of steps; each message of that method takes the next one
(the last repeats). A step may hold: "result" (answer it), "echo" (answer a hover with the
position it asked for), "hold" (answer nothing yet), "release" (first answer every held
request with this result), "notify" (messages to send after), "raw" (bytes to write), "exit",
"ignore". A top-level "linger" outlives its input closing. It writes its pid to .fake.pid
and the initializationOptions it was given to .fake.init.
"$ROOT" in what is sent is the root uri the client gave.
"""

import json
import os
import sys
import time


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))


ROOT = [""]


def write(message):
    body = json.dumps(message).replace("$ROOT", ROOT[0]).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def main():
    with open(sys.argv[1]) as f:
        script = json.load(f)
    with open(".fake.pid", "w") as f:
        f.write(str(os.getpid()))
    taken, held = {}, []
    while True:
        message = read()
        if message is None:
            if script.get("linger"):
                time.sleep(60)
            return 0
        method = message.get("method")
        if method == "initialize":
            ROOT[0] = message["params"]["rootUri"]
            with open(".fake.init", "w") as f:
                json.dump(message["params"].get("initializationOptions"), f)
        steps = script.get(method)
        if method == "exit" and not steps:
            return 0
        if not steps:
            if "id" in message and method is not None:
                write({"jsonrpc": "2.0", "id": message["id"], "result": None})
            continue
        n = taken.get(method, 0)
        taken[method] = n + 1
        step = steps[min(n, len(steps) - 1)]
        if step.get("ignore"):
            continue
        if "exit" in step:
            return step["exit"]
        if "raw" in step:
            sys.stdout.buffer.write(step["raw"].encode())
            sys.stdout.buffer.flush()
        if "release" in step:
            for old in held:
                write({"jsonrpc": "2.0", "id": old, "result": step["release"]})
            held.clear()
        if step.get("hold"):
            held.append(message["id"])
        elif step.get("echo"):
            at = message["params"]["position"]
            text = "line %d character %d" % (at["line"], at["character"])
            write({"jsonrpc": "2.0", "id": message["id"],
                   "result": {"contents": {"kind": "markdown", "value": text}}})
        elif "result" in step and "id" in message:
            write({"jsonrpc": "2.0", "id": message["id"], "result": step["result"]})
        for note in step.get("notify", []):
            write(note)


if __name__ == "__main__":
    sys.exit(main())
