"""A scripted debug adapter for tests: `replay <file>` plays a recorded exchange back,
`record <file> <adapter...>` sits between the client and a real adapter and writes one."""
import json
import subprocess
import sys
import threading


def read(stream):
    length = None
    while True:
        line = stream.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            length = int(value)
    if length is None:
        return None
    body = stream.read(length)
    return json.loads(body) if len(body) == length else None


def write(stream, msg):
    body = json.dumps(msg).encode()
    stream.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    stream.flush()


def replay(path):
    inp, out = sys.stdin.buffer, sys.stdout.buffer
    with open(path) as f:
        steps = [json.loads(line) for line in f if line.strip()]
    for n, step in enumerate(steps, 1):
        kind = step["dir"]
        if kind == "send":
            got, want = read(inp), step["msg"]
            if got is None or (got.get("seq"), got.get("command")) != (want["seq"], want["command"]):
                sys.stderr.write(f"step {n}: wanted {want['command']} #{want['seq']}, got {got}\n")
                sys.exit(3)
        elif kind == "recv":
            write(out, step["msg"])
        elif kind == "raw":
            out.write(step["text"].encode())
            out.flush()
        elif kind == "exit":
            sys.exit(step.get("code", 1))
    while read(inp) is not None:
        pass


def record(path, command):
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    log, lock = open(path, "w"), threading.Lock()

    def note(kind, msg):
        with lock:
            log.write(json.dumps({"dir": kind, "msg": msg}) + "\n")
            log.flush()

    def up():
        while (msg := read(child.stdout)) is not None:
            note("recv", msg)
            write(sys.stdout.buffer, msg)

    threading.Thread(target=up, daemon=True).start()
    while (msg := read(sys.stdin.buffer)) is not None:
        note("send", msg)
        write(child.stdin, msg)
    child.stdin.close()
    child.wait(5)


if __name__ == "__main__":
    if sys.argv[1] == "replay":
        replay(sys.argv[2])
    else:
        record(sys.argv[2], sys.argv[3:])
