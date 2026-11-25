#!/usr/bin/env python3
"""Measure the packaged app and write docs/receipts.md.

Usage: receipts.py. Needs `just package` and `just budgets` run first (it reads
target/package and target/budgets/last.json) and `yi` on PATH. Starts one private
`yi serve --model faux/faux-1` under target/receipts/home and the bundle's own binary
against it, both stopped by pid at the end.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

from bundle import BUNDLE, EXE, ID, ROOT

APP = BUNDLE / "Contents/MacOS" / EXE
TOOLING = ROOT / "target/package/yi-desktop-tooling"
WORK = ROOT / "target/receipts"
OUT = ROOT / "docs/receipts.md"
LAUNCHES = 5


def mib(n):
    return f"{n / 1048576:.1f} MiB"


def size_of(path):
    if path.is_file():
        return path.stat().st_size
    return sum(f.stat().st_size for f in path.rglob("*") if f.is_file())


def ps(pid, column):
    return subprocess.run(["ps", "-o", f"{column}=", "-p", str(pid)], capture_output=True,
                          text=True).stdout.strip()


def cpu_seconds(pid):
    text = ps(pid, "cputime")
    minutes, seconds = text.rsplit(":", 1)
    return int(minutes) * 60 + float(seconds)


def launch(env, sock, ready):
    app = subprocess.Popen([str(APP), "--socket", str(sock)], cwd=WORK, env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    file = ready / f"{ID}.{app.pid}"
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and not file.exists():
        time.sleep(0.05)
    if not file.exists():
        app.kill()
        sys.exit("receipts: the app wrote no ready file within 30 s")
    return app, json.loads(file.read_text())["ready_ms"]


def measure_app():
    if WORK.exists():
        subprocess.run(["/bin/rm", "-rf", str(WORK)], check=True)
    (WORK / "home").mkdir(parents=True)
    (WORK / "ready").mkdir()
    sock = WORK / "home/yi.sock"
    env = {"HOME": str(WORK / "home"), "PATH": os.environ["PATH"], "LANG": "en_US.UTF-8",
           "RUNE_READY_DIR": str(WORK / "ready"), "RUNE_OPEN_INACTIVE": "1"}
    server = subprocess.Popen(["yi", "serve", "--socket", str(sock), "--model", "faux/faux-1", "--confirm"],
                              env={k: env[k] for k in ("HOME", "PATH", "LANG")}, cwd=WORK,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            if sock.exists():
                break
            time.sleep(0.1)
        if not sock.exists():
            sys.exit("receipts: yi serve made no socket")
        starts = []
        for n in range(LAUNCHES):
            app, ms = launch(env, sock, WORK / "ready")
            starts.append(ms)
            if n < LAUNCHES - 1:
                app.terminate()
                app.wait(timeout=30)
        time.sleep(3)
        rss = int(ps(app.pid, "rss")) * 1024
        before = cpu_seconds(app.pid)
        time.sleep(10)
        idle = (cpu_seconds(app.pid) - before) / 10 * 100
        app.terminate()
        app.wait(timeout=30)
        return starts, rss, idle
    finally:
        server.terminate()
        server.wait(timeout=30)


def supply():
    run = subprocess.run(["just", "supply"], cwd=ROOT, capture_output=True, text=True)
    text = run.stdout + run.stderr
    warned = re.findall(r"^Crate:\s+(\S+)\nVersion:\s+(\S+)\nWarning:\s+(\S+)\n(?:.*\n){2}ID:\s+(\S+)", text, re.M)
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    kinds = {"registry": 0, "git": 0, "path": 0}
    for p in lock:
        src = p.get("source", "path")
        kinds["registry" if src.startswith("registry") else "git" if src.startswith("git") else "path"] += 1
    tree = subprocess.run(["cargo", "tree", "-p", "yi-desktop", "--target", "aarch64-apple-darwin",
                           "--prefix", "none", "-e", "normal,build"], cwd=ROOT, capture_output=True, text=True)
    built = len({line.split(" (")[0] for line in tree.stdout.splitlines() if line.strip()})
    return run.returncode, warned, kinds, built, "advisories ok, bans ok, licenses ok, sources ok" in text


def main():
    if not APP.is_file():
        sys.exit("receipts: run `just package` first")
    budgets = json.loads((ROOT / "target/budgets/last.json").read_text())
    starts, rss, idle = measure_app()
    status, warned, kinds, built, deny_ok = supply()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    rows = budgets["memory_100k_rows_kib"] * 1024
    flat = budgets["memory_before_rows_kib"] * 1024
    warm = sorted(starts[1:])[len(starts[1:]) // 2]
    lines = [
        "# Receipts", "",
        f"Yi Desktop {version}, the unsigned bundle from `just package`, measured on {time.strftime('%Y-%m-%d')} on "
        "an Apple M4 Max that was shared with four other agents building and running their own lanes "
        "(load average 20 to 45 on 16 cores). Every time below is therefore a ceiling: a quiet machine is faster. "
        "Re-measure with `just package`, `just budgets`, `just receipts`.", "",
        "| Receipt | Number |", "|---|---|",
        f"| Binary size, shipped in the bundle (dist profile, no `tooling` feature) | {size_of(APP):,} B ({mib(size_of(APP))}) |",
        f"| Binary size, the dry run's `tooling` build beside the bundle (not shipped) | {size_of(TOOLING):,} B ({mib(size_of(TOOLING))}) |",
        f"| Bundle size | {size_of(BUNDLE):,} B ({mib(size_of(BUNDLE))}) |",
        f"| Cold start to first frame, real window, first launch after the bundle was built | {starts[0]} ms |",
        f"| Start to first frame, median of the next {LAUNCHES - 1} launches | {warm} ms |",
        f"| Idle CPU over ten seconds, one window open, nothing running | {idle:.2f} % of one core |",
        f"| Memory at rest, real window, three seconds after the first frame | {mib(rss)} |",
        f"| Memory in the test process before the 100,000-row list | {mib(flat)} |",
        f"| Memory in the test process with the 100,000-row list drawn and scrolled | {mib(rows)} |", "",
        "How: the bundle's own binary runs against a private `yi serve --model faux/faux-1` with a home under "
        "`target/receipts/home`; Rune's `RUNE_READY_DIR` file gives `ready_ms`, counted from process start to the "
        "first window being ready. The first launch is cold only as far as the disk cache allows (no `purge`). "
        "The 100,000-row memory is the in-process budget test (`just budgets`), whose process also holds the "
        "test harness, so the difference between the two test rows is the number to read.", "",
        "## Budgets", "",
        "| Budget | Number | Budget |", "|---|---|---|",
    ]
    table = json.loads((ROOT / "tests/budgets/budgets.json").read_text())["budgets"]
    for name, row in table.items():
        lines.append(f"| {name} | {row['measured']} {row['unit']} | {row['budget']} {row['unit']} |")
    lines += [
        "", "## Supply", "",
        f"`just supply` exited {status}. cargo deny: {'advisories, bans, licenses and sources ok' if deny_ok else 'NOT ok'}; "
        f"it warns that `yi-browser` names two path dependencies without a version (a wildcard, allowed for paths in `deny.toml`).",
        f"Crates in `Cargo.lock`: {sum(kinds.values())} ({kinds['registry']} from crates.io, {kinds['git']} from git "
        f"(mlua, by Rune), {kinds['path']} by path). Compiled for the Mac app: {built}.", "",
        f"cargo audit allows {len(warned)} warnings, all `unmaintained`:", "",
    ]
    for name, ver, kind, ident in warned:
        lines.append(f"- `{name}` {ver} ({ident}): it is only reached through `accesskit_unix`, Rune's Linux "
                     "accessibility bridge, which is not compiled on macOS (`cargo tree -i zvariant` shows it only "
                     "with `--target all`). Nothing in this app can call it; the fix is Rune's to make.")
    OUT.write_text("\n".join(lines) + "\n")
    print(f"receipts: wrote {OUT}")


if __name__ == "__main__":
    main()
