#!/usr/bin/env python3
"""The daily-loop journeys: the real app, connected to a private `yi serve`, driven headless.

Usage: run.py [NAME ...] (default: every tests/journeys/daily/fixtures/*.json). Each journey runs in
target/journey/daily-<name>/ with its own HOME, socket and git project, on the offline
faux/faux-1 model. A journey is `seed` (threads made over the wire before the app starts),
`worker` (true swaps the project's worker for faux-worker.sh beside this file, whose turns ask to
run a command), and `runs`: one app launch each, with Rune `--script` `steps` and an `expect`
of `keys`, `texts` (whole element texts) and `contains` (a piece of some element's text) the
final tree must hold, and `files` the project must hold. Every prompt after the seed is typed
into the app's own composer. Exit 0 when every journey passed.
"""

import json
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT / "tools/harness"))
import journey as harness  # noqa: E402

WORKER = HERE / "faux-worker.sh"
TURN = 60.0


class Wire:
    """One JSON-RPC connection to the server, as another client would hold it."""

    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(str(path))
        self.buf = b""
        self.n = 0
        self.request("initialize", {"protocolVersion": 2,
                                    "clientInfo": {"name": "journey", "version": "0"}})

    def send(self, method, params):
        self.n += 1
        line = {"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}
        self.sock.sendall(json.dumps(line).encode() + b"\n")
        return self.n

    def frame(self, deadline):
        while b"\n" not in self.buf:
            left = deadline - time.monotonic()
            if left <= 0:
                raise RuntimeError("the server went quiet")
            self.sock.settimeout(left)
            more = self.sock.recv(65536)
            if not more:
                raise RuntimeError("the server closed the socket")
            self.buf += more
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line)

    def until(self, done, timeout=TURN):
        deadline = time.monotonic() + timeout
        while True:
            frame = self.frame(deadline)
            if done(frame):
                return frame

    def request(self, method, params):
        n = self.send(method, params)
        frame = self.until(lambda f: f.get("id") == n and "method" not in f)
        if "error" in frame:
            raise RuntimeError(f"{method}: {frame['error']}")
        return frame.get("result")

    def thread(self, cwd, prompt):
        session = self.request("session/new", {"cwd": str(cwd), "mcpServers": []})["sessionId"]
        self.send("session/prompt", {"sessionId": session,
                                     "prompt": [{"type": "text", "text": prompt}]})
        self.until(idle)
        return session

    def close(self):
        self.sock.close()


def idle(frame):
    update = frame.get("params", {}).get("update", {})
    return update.get("sessionUpdate") == "state_update" and update.get("state") == "idle"


def project(work):
    folder = work / "project"
    folder.mkdir()
    git = ["git", "-C", str(folder)]
    subprocess.run(git + ["init", "-q", "-b", "main"], check=True)
    (folder / "README").write_text("seed\n")
    subprocess.run(git + ["add", "README"], check=True)
    subprocess.run(git + ["-c", "user.name=journey", "-c", "user.email=journey@example.invalid",
                          "commit", "-qm", "Seed"], check=True)
    return folder


def rig(wire, folder):
    """Swaps the project's worker for the cassette one, as the wire spike does."""
    wire.thread(folder, "warm up")
    deadline = time.monotonic() + 10
    while True:
        try:
            wire.request("_yi/worker_replace", {"cwd": str(folder), "binary": str(WORKER)})
            return
        except RuntimeError:
            if time.monotonic() > deadline:
                raise
            time.sleep(0.2)


def serve(work, home, sock, flags):
    """`yi serve` on the offline model, as the harness starts it, with this journey's mode."""
    yi = shutil.which("yi")
    if yi is None:
        raise RuntimeError("yi is not on PATH")
    log = open(work / "serve.log", "w")
    server = subprocess.Popen([yi, "serve", "--socket", str(sock), "--model", "faux/faux-1"] + flags,
                              env=harness.environment(home), stdout=log,
                              stderr=subprocess.STDOUT, cwd=work)
    deadline = time.monotonic() + harness.SOCKET_WAIT
    while not sock.exists():
        if server.poll() is not None or time.monotonic() > deadline:
            harness.stop_server(server)
            raise RuntimeError(f"yi serve gave no socket; see {work / 'serve.log'}")
        time.sleep(0.05)
    return server


def check(expect, tree, folder):
    problems = harness.check({"expect": expect}, tree)
    texts = [e.get("text") or "" for e in tree.get("elements", [])]
    problems += [f"no element holds {c!r}" for c in expect.get("contains", [])
                 if not any(c in t for t in texts)]
    problems += [f"the project has no {f}" for f in expect.get("files", [])
                 if not (folder / f).exists()]
    problems += [f"the project has {f}" for f in expect.get("absent", []) if (folder / f).exists()]
    return problems


def launch(work, home, sock, run, n):
    out = work / f"out/run{n}"
    out.mkdir(parents=True)
    steps = work / f"steps{n}.json"
    # A headless realtime snap keeps the last drawn pixels, and a wake alone draws none.
    end = max(step["t"] for step in run["steps"])
    nudge = {"t": max(end - 0.3, 0.0), "type": "move", "x": 5, "y": 745}
    steps.write_text(json.dumps(sorted(run["steps"] + [nudge], key=lambda step: step["t"])))
    log = open(work / f"app{n}.log", "w")
    app = subprocess.Popen(
        [str(harness.BIN), "--realtime", "--script", str(steps), "--snap", str(out / "end.png"),
         "--tree", str(out / "end.tree.json"), "--socket", str(sock)],
        env=harness.environment(home), cwd=work, stdout=log, stderr=subprocess.STDOUT)
    try:
        code = app.wait(timeout=harness.APP_TIMEOUT)
    except subprocess.TimeoutExpired:
        code = "timeout"
    finally:
        if app.poll() is None:
            app.kill()
            app.wait()
        log.close()
    if code != 0:
        return None, [f"run {n}: app exit {code}; see {work / f'app{n}.log'}"]
    return json.loads((out / "end.tree.json").read_text()), []


def run(name):
    journey = json.loads((HERE / "fixtures" / f"{name}.json").read_text())
    work = ROOT / "target/journey" / f"daily-{name}"
    if work.exists():
        shutil.rmtree(work)
    home = work / "home"
    home.mkdir(parents=True)
    folder = project(work)
    sock = home / "yi.sock"
    server = serve(work, home, sock, journey.get("serve", ["--confirm"]))
    problems = []
    try:
        wire = Wire(sock)
        if journey.get("worker"):
            rig(wire, folder)
        for prompt in journey.get("seed", []):
            wire.thread(folder, prompt)
        if journey.get("empty"):
            wire.request("session/new", {"cwd": str(folder), "mcpServers": []})
        for n, step in enumerate(journey["runs"]):
            tree, failed = launch(work, home, sock, step, n)
            problems += failed or [f"run {n}: {p}" for p in check(step["expect"], tree, folder)]
        wire.close()
    finally:
        harness.stop_server(server)
    time.sleep(1)
    stray = harness.left_running(work)
    return problems + ([f"left running: {stray}"] if stray else [])


def main():
    names = sys.argv[1:] or sorted(p.stem for p in (HERE / "fixtures").glob("*.json"))
    built = subprocess.run(["cargo", "build", "-q", "-p", "yi-desktop"], cwd=ROOT,
                           capture_output=True, text=True)
    if built.returncode != 0:
        print(f"daily: build failed: {built.stderr.strip().splitlines()[-5:]}")
        return 1
    failed = 0
    for name in names:
        try:
            problems = run(name)
        except (OSError, RuntimeError, KeyError, ValueError, subprocess.SubprocessError) as error:
            problems = [str(error)]
        failed += bool(problems)
        for p in problems:
            print(f"daily: {name}: {p}")
        print(f"daily: {name}: {'failed' if problems else 'ok'} (target/journey/daily-{name}/out)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
