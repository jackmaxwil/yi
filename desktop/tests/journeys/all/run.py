#!/usr/bin/env python3
"""The wider journeys: every pane, a watched turn, code tools, a server that goes and returns.

Usage: run.py [NAME ...] (default: every tests/journeys/all/fixtures/*.json). Built on the daily runner:
a journey is its `seed`, `worker`, `empty`, `serve` and `runs`, and a run adds `size` (the
window, "WxH"), `shims` (programs written into a folder put first on the app's PATH, each a
shell body), `toolchain` (true: the real Rust toolchain is read, `RUSTUP_HOME` passed through), `nudge` (where
the pointer goes just before the end, [5, 745] unless set: a
rest it must not end, such as a hover card's) and `server` (what the runner does to the server while the app runs: a list of
{"after": <state>, "do": "stop" | "start"}, the state one of "connected" or "stopped" or
"started"). `{PROJECT}` in a step's text is the project folder, and `expect.project_threads`
is how many threads the server lists in that folder once the run ends. Every wait here is on a state the runner can see, the server's own clients or its
socket; only the app's script is timed, as Rune's scripts are. The runner starts each process
as the leader of its own group and fails a journey if any pid it started, or anything in that
group, is still alive. It never touches ~/.yi: HOME is the journey's own folder.
"""

import importlib.util
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
spec = importlib.util.spec_from_file_location("daily_run", ROOT / "tests/journeys/daily/run.py")
daily = importlib.util.module_from_spec(spec)
spec.loader.exec_module(daily)
harness = daily.harness

STATE_WAIT = 20.0


class Started:
    """Every process this journey started, by pid, each the leader of its own group.

    A group and not a session: a session leader that opens the shell's pty takes it as its
    controlling terminal and is hung up when the shell ends, which killed the app at exit.
    """

    def __init__(self):
        self.pids = []

    def popen(self, argv, **kw):
        child = subprocess.Popen(argv, process_group=0, **kw)
        self.pids.append(child.pid)
        return child

    def survivors(self):
        found = []
        for pid in self.pids:
            group = subprocess.run(["pgrep", "-g", str(pid)], capture_output=True, text=True)
            found += [int(p) for p in group.stdout.split()]
        return found

    def reap(self):
        """Kills, by exact pid, what is left; returns those pids."""
        left = self.survivors()
        for pid in left:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        return left


def clients(server_pid, sock):
    """How many connections the server holds on its socket (its own listener excluded).

    A connection it accepted is listed under the socket's path, as the listener is. The `->`
    lines are pairs of its own, four with nobody connected: counting those stopped the server
    before a slow app had connected, and the app then never drew Reconnect.
    """
    out = subprocess.run(["lsof", "-a", "-U", "-p", str(server_pid), "-F", "n"],
                         capture_output=True, text=True).stdout
    return sum(1 for line in out.splitlines() if line == f"n{sock}") - 1


def until(what, test, timeout=STATE_WAIT):
    deadline = time.monotonic() + timeout
    while not test():
        if time.monotonic() > deadline:
            raise RuntimeError(f"gave up waiting for {what}")
        time.sleep(0.05)


class Rig:
    """The server of one journey, which the journey may stop and start again."""

    def __init__(self, work, home, sock, flags, started):
        self.work, self.home, self.sock, self.flags, self.started = work, home, sock, flags, started
        self.server = None

    def start(self):
        if self.sock.exists():
            self.sock.unlink()
        self.sock.with_suffix(".stopped").unlink(missing_ok=True)
        log = open(self.work / "serve.log", "a")
        yi = shutil.which("yi")
        self.server = self.started.popen(
            [yi, "serve", "--socket", str(self.sock), "--model", "faux/faux-1"] + self.flags,
            env=harness.environment(self.home), stdout=log, stderr=subprocess.STDOUT, cwd=self.work)
        until("the socket", lambda: self.sock.exists() or self.server.poll() is not None)
        if self.server.poll() is not None:
            raise RuntimeError(f"yi serve gave no socket; see {self.work / 'serve.log'}")

    def stop(self):
        """Stops it on purpose: the marker the app's client reads, so it does not start another."""
        self.sock.with_suffix(".stopped").write_text("stopped by the journey\n")
        harness.stop_server(self.server)
        until("the server to end", lambda: self.server.poll() is not None)

    def connected(self):
        return clients(self.server.pid, self.sock) > 0


def shim_env(work, home, shims, toolchain=False):
    env = harness.environment(home)
    # A run that uses the real Rust toolchain reads it where it is installed; it writes nothing.
    if toolchain:
        real = Path(os.path.expanduser("~"))
        env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(real / ".rustup"))
    if shims:
        folder = work / "bin"
        folder.mkdir(exist_ok=True)
        for name, body in shims.items():
            path = folder / name
            path.write_text("#!/bin/sh\n" + body.replace("{ROOT}", str(ROOT)) + "\n")
            path.chmod(0o755)
        env["PATH"] = f"{folder}:{env['PATH']}"
    return env


def act(rig, actions):
    """Does what the journey asks of the server, each once its state is seen."""
    for step in actions:
        if step["after"] == "connected":
            until("the app to connect", rig.connected)
        elif step["after"] == "stopped":
            until("the server to be gone", lambda: rig.server.poll() is not None)
        time.sleep(step.get("settle", 0))
        {"stop": rig.stop, "start": rig.start}[step["do"]]()


def bodies(expect, tree):
    """Each pane key in `bodies` draws words below its tab bar: never a blank body.

    `absent_keys` and `absent_texts` must be missing from the tree.
    """
    elements = tree.get("elements", [])
    keys = {e.get("key") for e in elements}
    texts = {e.get("text") for e in elements}
    found = [f"{k} is there" for k in expect.get("absent_keys", []) if k in keys]
    found += [f"{t!r} is shown" for t in expect.get("absent_texts", []) if t in texts]
    for pane in expect.get("bodies", []):
        root = next((e for e in elements if e.get("key") == pane), None)
        bar = next((e for e in elements if e.get("key") == f"{pane}.pane-tab-bar"), None)
        if root is None or bar is None:
            found.append(f"no pane {pane}")
            continue
        x, y, w, h = root["rect"]
        below = bar["rect"][1] + bar["rect"][3]
        words = [e for e in elements if e.get("text", "").strip() and not e.get("exiting")
                 and x <= e["rect"][0] <= x + w and below <= e["rect"][1] <= y + h]
        found += [] if words else [f"{pane} draws a blank body"]
    return found


def project_threads(expect, rig, folder):
    """The server's own list of the project's threads holds as many as the journey expects."""
    want = expect.get("project_threads")
    if want is None:
        return []
    wire = daily.Wire(rig.sock)
    try:
        listed = wire.request("session/list", {"cwd": str(folder)}).get("sessions", [])
    finally:
        wire.close()
    here = [s for s in listed if s.get("cwd") == str(folder)]
    return [] if len(here) == want else [f"the server lists {len(here)} threads in the project, not {want}"]


def launch(work, home, rig, run, n, started, folder):
    out = work / f"out/run{n}"
    out.mkdir(parents=True)
    end = max(step["t"] for step in run["steps"])
    x, y = run.get("nudge", [5, 745])
    nudge = {"t": max(end - 0.3, 0.0), "type": "move", "x": x, "y": y}
    steps = work / f"steps{n}.json"
    script = json.dumps(sorted(run["steps"] + [nudge], key=lambda step: step["t"]))
    steps.write_text(script.replace("{PROJECT}", str(folder)))
    argv = [str(harness.BIN), "--realtime", "--script", str(steps), "--snap", str(out / "end.png"),
            "--tree", str(out / "end.tree.json"), "--socket", str(rig.sock)]
    if run.get("size"):
        argv += ["--size", run["size"]]
    log = open(work / f"app{n}.log", "w")
    app = started.popen(argv, env=shim_env(work, home, run.get("shims"), run.get("toolchain", False)), cwd=work,
                        stdout=log, stderr=subprocess.STDOUT)
    problems = []
    try:
        act(rig, run.get("server", []))
        code = app.wait(timeout=harness.APP_TIMEOUT)
    except subprocess.TimeoutExpired:
        code = "timeout"
    except RuntimeError as error:
        code, problems = "stopped early", [f"run {n}: {error}"]
    finally:
        if app.poll() is None:
            app.kill()
            app.wait()
        log.close()
    if code != 0:
        return None, problems + [f"run {n}: app exit {code}; see {work / f'app{n}.log'}"]
    return json.loads((out / "end.tree.json").read_text()), problems


def run(name):
    journey = json.loads((HERE / "fixtures" / f"{name}.json").read_text())
    work = ROOT / "target/journey" / f"all-{name}"
    if work.exists():
        shutil.rmtree(work)
    home = work / "home"
    home.mkdir(parents=True)
    folder = daily.project(work)
    for path, text in journey.get("project", {}).items():
        (folder / path).parent.mkdir(parents=True, exist_ok=True)
        (folder / path).write_text(text)
        git = ["git", "-C", str(folder), "-c", "user.name=journey", "-c", "user.email=journey@example.invalid"]
        subprocess.run(git + ["add", path], check=True)
        subprocess.run(git + ["commit", "-qm", f"Add {path}"], check=True)
    started = Started()
    rig = Rig(work, home, home / "yi.sock", journey.get("serve", ["--confirm"]), started)
    problems = []
    try:
        rig.start()
        wire = daily.Wire(rig.sock)
        if journey.get("worker"):
            daily.rig(wire, folder)
        for prompt in journey.get("seed", []):
            wire.thread(folder, prompt)
        if journey.get("empty"):
            wire.request("session/new", {"cwd": str(folder), "mcpServers": []})
        wire.close()
        for path, lines in journey.get("files", {}).items():
            target = home / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("\n".join(json.dumps(l) if isinstance(l, dict) else l for l in lines) + "\n")
        for n, step in enumerate(journey["runs"]):
            tree, failed = launch(work, home, rig, step, n, started, folder)
            problems += failed
            if tree is not None:
                found = daily.check(step["expect"], tree, folder) + bodies(step["expect"], tree)
                found += project_threads(step["expect"], rig, folder)
                problems += [f"run {n}: {p}" for p in found]
    finally:
        if rig.server and rig.server.poll() is None:
            harness.stop_server(rig.server)
        left = started.reap()
    return problems + ([f"left running, killed by pid: {left}"] if left else [])


def main():
    names = sys.argv[1:] or sorted(p.stem for p in (HERE / "fixtures").glob("*.json"))
    built = subprocess.run(["cargo", "build", "-q", "-p", "yi-desktop"], cwd=ROOT,
                           capture_output=True, text=True)
    if built.returncode != 0:
        print(f"all: build failed: {built.stderr.strip().splitlines()[-5:]}")
        return 1
    failed, times = 0, {}
    for name in names:
        began = time.monotonic()
        try:
            problems = run(name)
        except (OSError, RuntimeError, KeyError, ValueError, subprocess.SubprocessError) as error:
            problems = [str(error)]
        times[name] = round(time.monotonic() - began, 1)
        failed += bool(problems)
        for p in problems:
            print(f"all: {name}: {p}")
        print(f"all: {name}: {'failed' if problems else 'ok'} in {times[name]} s "
              f"(target/journey/all-{name}/out)")
    out = ROOT / "target/journey"
    out.mkdir(parents=True, exist_ok=True)
    (out / "all-times.json").write_text(json.dumps(times, indent=1))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
