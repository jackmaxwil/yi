#!/usr/bin/env python3
"""Journey runner: a private `yi serve`, the app driven headless by a script, its tree and picture.

Usage: journey.py [NAME ...] (default: every tools/harness/journeys/*.json, then the daily
journeys of tests/journeys/daily, then the wider ones of tests/journeys/all; a name found in
neither folder here is taken to be a daily one). Each journey runs
in target/journey/<name>/ with its own HOME and socket and the offline faux/faux-1 model; the
server is stopped by its own pid. Exit 0 when every journey met its expectations, 1 otherwise.
"""

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "target/debug/yi-desktop"
JOURNEYS = Path(__file__).resolve().parent / "journeys"
DAILY = ROOT / "tests/journeys/daily/run.py"
ALL = ROOT / "tests/journeys/all/run.py"
SOCKET_WAIT = 10.0
APP_TIMEOUT = 120


def environment(home):
    return {"HOME": str(home), "PATH": os.environ.get("PATH", ""), "LANG": "en_US.UTF-8"}


def start_server(work, home):
    yi = shutil.which("yi")
    if yi is None:
        raise RuntimeError("yi is not on PATH")
    socket = home / "yi.sock"
    log = open(work / "serve.log", "w")
    server = subprocess.Popen(
        [yi, "serve", "--socket", str(socket), "--model", "faux/faux-1", "--confirm"],
        env=environment(home), stdout=log, stderr=subprocess.STDOUT, cwd=work)
    deadline = time.monotonic() + SOCKET_WAIT
    while not socket.exists():
        if server.poll() is not None or time.monotonic() > deadline:
            stop_server(server)
            raise RuntimeError(f"yi serve gave no socket in {SOCKET_WAIT} s; see {work / 'serve.log'}")
        time.sleep(0.05)
    return server


def stop_server(server):
    if server.poll() is None:
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()


def left_running(work):
    found = subprocess.run(["pgrep", "-fl", str(work) + "/"], capture_output=True, text=True)
    return found.stdout.strip()


def check(journey, tree):
    elements = tree.get("elements", [])
    keys = {e.get("key") for e in elements}
    texts = {e.get("text") for e in elements}
    want = journey.get("expect", {})
    return ([f"no element keyed {k!r}" for k in want.get("keys", []) if k not in keys]
            + [f"no element shows {t!r}" for t in want.get("texts", []) if t not in texts])


def run(name):
    journey = json.loads((JOURNEYS / f"{name}.json").read_text())
    work = ROOT / "target/journey" / name
    if work.exists():
        shutil.rmtree(work)
    home, out = work / "home", work / "out"
    home.mkdir(parents=True)
    out.mkdir()
    steps = work / "steps.json"
    steps.write_text(json.dumps(journey["steps"]))
    server = start_server(work, home)
    try:
        app = subprocess.run(
            [str(BIN), "--scene", journey["scene"], "--script", str(steps),
             "--snap", str(out / "end.png"), "--tree", str(out / "end.tree.json")],
            env=environment(home), cwd=work, capture_output=True, text=True, timeout=APP_TIMEOUT)
    finally:
        stop_server(server)
    (work / "app.log").write_text(app.stdout + app.stderr)
    problems = [] if app.returncode == 0 else [f"app exit {app.returncode}; see {work / 'app.log'}"]
    tree = out / "end.tree.json"
    problems += check(journey, json.loads(tree.read_text())) if tree.exists() else ["no tree written"]
    stray = left_running(work)
    problems += [f"left running: {stray}"] if stray else []
    return problems


def main():
    asked = sys.argv[1:]
    here = sorted(p.stem for p in JOURNEYS.glob("*.json"))
    names = [name for name in asked if name in here] if asked else here
    wider = [name for name in asked if (ALL.parent / "fixtures" / f"{name}.json").exists()]
    daily = [name for name in asked if name not in here and name not in wider]
    built = subprocess.run(["cargo", "build", "-q", "-p", "yi-desktop"], cwd=ROOT,
                           capture_output=True, text=True)
    if built.returncode != 0:
        print(f"journey: build failed: {built.stderr.strip().splitlines()[-5:]}")
        return 1
    failed = 0
    for name in names:
        try:
            problems = run(name)
        except (OSError, RuntimeError, KeyError, ValueError, subprocess.TimeoutExpired) as error:
            problems = [str(error)]
        failed += bool(problems)
        for p in problems:
            print(f"journey: {name}: {p}")
        print(f"journey: {name}: {'failed' if problems else 'ok'} (target/journey/{name}/out)")
    if daily or not asked:
        failed += subprocess.run([sys.executable, str(DAILY), *daily], cwd=ROOT).returncode != 0
    if wider or not asked:
        failed += subprocess.run([sys.executable, str(ALL), *wider], cwd=ROOT).returncode != 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
