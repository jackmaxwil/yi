#!/usr/bin/env python3
"""Monkey runs: seeded random key scripts played through the real headless TUI loops.

  tui_see.py monkey [--runs N] [--seed S] [--surface solo|console] [--steps N] [--bin yi] [--out DIR]

Run i plays seed S+i. Each seed writes `<out>/<seed>/<surface>.drive`, a scene `tui_see.py run`
replays as it stands; only a run that crashes, hangs or leaves a control character in a cell is
reported, with its stderr beside it as `<surface>.err` and its frames kept. Green runs keep only
the script. The drive engine already refuses a control cell (`keep_frame`); this finds the keys
that reach one. Frame corruption without a control character (wrong rows, dropped text) is not caught.
"""
from __future__ import annotations

import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import tui_see

MASK = (1 << 64) - 1
SIZES = ["20x8", "24x10", "40x12", "60x12", "80x24", "120x34", "200x50"]
WAITS = [1, 5, 20, 50, 120]
HANG_SECS = 60
# A hung loop may never reach its own deadline check, so the runner kills it this much later.
GRACE = 30
WORDS = ["hello", "make the rail calmer", "/", "@", "!ls", "ok", "x" * 200, "易", "é",
         "\U0001F469‍\U0001F4BB", "日本語のテキスト", "a‮b", "​"]
# What a paste can carry that no key produces: the cells must never show these raw.
PASTED = ["\t", "\x07", "\x08", "\x1b[31mred", "\x7f", "\x00", "\x9b", " ", "﻿", "\x01\x02"]
SLASH = ["new", "undo", "tree", "editor", "advisor", "plan", "plantree", "goal", "agents", "model",
         "permissions", "compact", "sessions", "todo", "nope"]
FUNCTION = [f"f{n}" for n in range(1, 13)]
SOLO_KEYS = (["enter", "esc", "tab", "shift-tab", "backspace", "delete", "up", "down", "left",
              "right", "home", "end", "pageup", "pagedown", "space", "shift-enter", "alt-up",
              "alt-down", "alt-left", "alt-right", "ctrl-o", "ctrl-t", "ctrl-p", "alt-m", "ctrl-r",
              "ctrl-g", "ctrl-left", "ctrl-right", "alt-backspace"]
             + [f"ctrl-{c}" for c in "aeklwunfb"] + FUNCTION)
CONSOLE_KEYS = (["enter", "esc", "tab", "backspace", "up", "down", "left", "right", "pageup",
                 "pagedown", "alt-up", "alt-down", "alt-left", "alt-right", "alt-shift-j", "ctrl-o",
                 "ctrl-t", "ctrl-p", "ctrl-r"]
                + [f"alt-{c}" for c in "vsxztn/kjbgye?[]123456789"])
PREFIXED = list("vsxzhjklcnpog123456789")
CMD_KEYS = list("pbiryolsu/\\dx123456789") + ["shift-d", "shift-u", "shift-\\", "shift-m", "shift-n"]
MOUSE = ["down", "up", "drag", "scrollup", "scrolldown"]


class Rng:
    """splitmix64: tiny, and the same stream on every machine for a seed."""

    def __init__(self, seed: int):
        self.state = seed & MASK

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
        return z ^ (z >> 31)

    def below(self, n: int) -> int:
        return self.next() % n

    def pick(self, items: list):
        return items[self.below(len(items))]

    def weighted(self, table: list[tuple[str, int]]) -> str:
        roll = self.below(sum(weight for _, weight in table))
        for name, weight in table:
            if roll < weight:
                return name
            roll -= weight
        raise AssertionError("weights exhausted")


def text(rng: Rng) -> str:
    word = rng.pick(WORDS)
    return f"type {word}" if rng.below(3) else f"type {'/' + rng.pick(SLASH)}"


def step(rng: Rng, surface: str, width: int, height: int, pasting: bool) -> list[str]:
    kinds = [("key", 40), ("text", 25), ("wait", 8)] + [("paste", 3)] * pasting
    if surface == "console":
        kinds += [("prefix", 6), ("cmd", 8), ("mouse", 10)]
    kind = rng.weighted(kinds)
    if kind == "text":
        return [text(rng)]
    if kind == "wait":
        return [f"wait {rng.pick(WAITS)}"]
    if kind == "paste":
        return [f"type x{rng.pick(PASTED)}y"]
    if kind == "prefix":
        return ["key ctrl-b", f"key {rng.pick(PREFIXED)}"]
    if kind == "cmd":
        return [f"cmd-{rng.pick(CMD_KEYS)}"]
    if kind == "mouse":
        verb = rng.pick(MOUSE)
        return [f"mouse {verb} {rng.below(width)} {rng.below(height)}"]
    return [f"key {rng.pick(SOLO_KEYS if surface == 'solo' else CONSOLE_KEYS)}"]


def generate(seed: int, surface: str, steps: int = 40) -> str:
    rng = Rng(seed)
    size = rng.pick(SIZES)
    width, height = map(int, size.split("x"))
    flags = f"{size} --deadline {HANG_SECS}" if surface == "solo" else size
    lines = [f"# monkey seed={seed} surface={surface} steps={steps}", f"#! {surface} --size {flags}"]
    if surface == "console":
        lines += ["wait-frame 15000 !connecting…", "key alt-n", "wait-frame 8000 untitled"]
    lines += ["type hello", "key enter", "wait-frame 15000 faux:"]
    # One script in four pastes control characters; the rest stay clean so the keys can fail alone.
    pasting = rng.below(4) == 0
    for _ in range(steps):
        lines += step(rng, surface, width, height, pasting)
    lines.append("quit")
    return "\n".join(lines) + "\n"


def classify(code: int, err: str) -> str | None:
    """None for a green run; otherwise the failure kind a person reads first."""
    if code == 0:
        return None
    if "control character" in err:
        return "control character in a cell"
    if "drive script timed out" in err:
        return "hang: the drive deadline fired"
    if "timed out after" in err:
        return "hang: a wait never saw the frame it waits on"
    if "panicked" in err or code < 0 or code == 101:
        return "crash"
    if re.search(r"\bline \d+:", err):
        return "bad script: the generator wrote what the drive grammar refuses"
    return f"exit {code}"


def play_seed(seed: int, surface: str, steps: int, binary: Path, out: Path) -> str | None:
    base = out / str(seed)
    base.mkdir(parents=True, exist_ok=True)
    scene = base / f"{surface}.drive"
    scene.write_text(generate(seed, surface, steps))
    errfile = base / f"{surface}.err"
    run_dir = base / surface
    _, extra = tui_see.header(scene)
    kind = None
    with open(errfile, "w") as err:
        try:
            code, _ = tui_see.play(scene, surface, extra, run_dir, binary, record=False,
                                   timeout=HANG_SECS + GRACE, stderr=err)
        except subprocess.TimeoutExpired:
            code = None
            kind = f"hang: killed after {HANG_SECS + GRACE} s"
    if kind is None:
        kind = classify(code, errfile.read_text())
    if kind is None:
        shutil.rmtree(run_dir, ignore_errors=True)
        errfile.unlink()
    return kind


def monkey(argv: list[str]) -> None:
    opts = dict(zip(argv[::2], argv[1::2]))
    unknown = set(opts) - {"--runs", "--seed", "--surface", "--steps", "--bin", "--out"}
    if unknown or len(argv) % 2:
        sys.exit(f"monkey: unknown or valueless flags {sorted(unknown) or argv[-1:]}")
    runs, steps = int(opts.get("--runs", 5)), int(opts.get("--steps", 40))
    seed = int(opts.get("--seed", time.time_ns() % 1_000_000))
    surfaces = [opts["--surface"]] if "--surface" in opts else ["solo", "console"]
    if not set(surfaces) <= {"solo", "console"}:
        sys.exit(f"monkey: --surface is solo or console, not {surfaces[0]!r}")
    binary = Path(opts.get("--bin", "target/debug/yi")).resolve()
    out = Path(opts.get("--out", "target/see/monkey")).resolve()
    failed = []
    for seed_i in range(seed, seed + runs):
        for surface in surfaces:
            kind = play_seed(seed_i, surface, steps, binary, out)
            if kind:
                base = out / str(seed_i)
                failed.append(seed_i)
                tail = (base / f"{surface}.err").read_text().strip().splitlines()[-3:]
                print(f"FAIL seed {seed_i} {surface}: {kind}")
                print("\n".join(f"    {line}" for line in tail))
                print(f"    replay: python3 scripts/tui_see.py run {base / (surface + '.drive')}")
    print(f"monkey: {runs} seeds from {seed} on {', '.join(surfaces)}, {len(failed)} failed")
    if failed:
        sys.exit(1)


def selfcheck() -> None:
    # Vigna's published first output for seed 0: the stream is the reference's, not ours.
    assert Rng(0).next() == 0xE220A8397B1DCDAF, "splitmix64 drifted from the reference"
    for surface in ("solo", "console"):
        assert generate(7, surface) == generate(7, surface), "a seed must replay byte for byte"
        scripts = {generate(seed, surface).split("\n", 1)[1] for seed in range(20)}
        assert len(scripts) == 20, "distinct seeds must write distinct scripts"
    verbs = {"key", "type", "wait", "wait-frame", "quit", "mouse"}
    for surface in ("solo", "console"):
        for seed in range(30):
            body = generate(seed, surface).split("\n")[:-1]
            assert body[1].startswith(f"#! {surface} --size "), body[1]
            assert body[-1] == "quit", "a script must end"
            for line in body[2:]:
                assert line.split(" ")[0] in verbs or line.startswith("cmd-"), line
    # The classifier is the only thing standing between a crash and a green report.
    assert classify(0, "") is None
    assert classify(1, "error: a control character reached a cell: ...") == "control character in a cell"
    assert classify(1, "error: drive script timed out (60 s wall)").startswith("hang")
    assert classify(1, "error: wait-frame timed out after 15000 ms").startswith("hang")
    assert classify(101, "thread 'main' panicked at x").startswith("crash")
    assert classify(3, "") == "exit 3", "an unrecognised failure must still be a failure"
    assert classify(2, "error: line 3: unknown step: keyy").startswith("bad script")
    with tempfile.TemporaryDirectory() as tmp:
        scene = Path(tmp) / "s.drive"
        scene.write_text(generate(1, "console"))
        assert tui_see.header(scene)[0] == "console"
    print("tui_monkey selfcheck: ok")


if __name__ == "__main__":
    if sys.argv[1:] != ["--selfcheck"]:
        sys.exit("tui_monkey.py --selfcheck, or run it as `tui_see.py monkey`")
    selfcheck()
