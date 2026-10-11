#!/usr/bin/env python3
"""Run a TUI scene headless, keep its named snaps, and diff two runs so an agent can see a change.

A scene is a drive script (crates/tui/src/drive.rs grammar) whose `#!` lines carry its setup:
the surface first, then flags, e.g. `#! solo --size 80x24 --faux replies.jsonl --seed s.jsonl`
or `#! console --size 120x34`. `snap <name>` steps mark the frames worth looking at.

  tui_see.py run SCENE [--label head] [--out target/see] [--bin target/debug/yi]
  tui_see.py diff RUN_A RUN_B          # two run dirs; writes RUN_B/diff-vs-<A>/summary.md
  tui_see.py --selfcheck

What an agent reads, in order: the cell diff as text (exact), then 1:1 crops of each changed
region before and after, then the whole frame. Pictures never gate anything.
"""
from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "scripts" / "proof" / "tui_see"
# Loud caps: a cut names what it kept, the cap and the call that gets the rest (045-loud-caps).
MAX_REGIONS = 6
MAX_CROP_PX = 1200
MARGIN_ROWS, MARGIN_COLS = 2, 8


def header(scene: Path) -> tuple[str, list[str]]:
    words = []
    for line in scene.read_text().splitlines():
        if line.startswith("#!"):
            words += shlex.split(line[2:])
    surface = words.pop(0) if words and words[0] in ("solo", "console") else "solo"
    return surface, words


def encoded(cwd: Path) -> str:
    return "--" + str(cwd).strip("/").replace("/", "-").replace(":", "-") + "--"


NAME = re.compile(r"^[a-z0-9_-]+$")
FIXED_ID = "01a00000-0000-7000-8000-000000000000"


def run(scene: Path, label: str, out: Path, binary: Path) -> Path:
    surface, extra = header(scene)
    if not NAME.match(label):
        sys.exit(f"label {label!r} is not [a-z0-9_-]+")
    run_dir = (out / scene.stem / label).resolve()
    shutil.rmtree(run_dir, ignore_errors=True)
    for sub in ("frames", "sessions", "home/.yi"):
        (run_dir / sub).mkdir(parents=True)
    # Incident: a console scene's session claimed a lane, a `git worktree add` in the shared
    # .git that outlived the run dir and refused the next run as "missing but locked".
    (run_dir / "home/.yi/config.json").write_text('{"kernel":{"prewarm":false},"lanes":{"enabled":false}}')
    env = {**os.environ, "HOME": str(run_dir / "home")}
    drive = ["--keys", str(scene.resolve()), "--frames", str(run_dir / "frames"),
             "--record", str(run_dir / "run.cast")]
    if "--seed" in extra and surface == "console":
        sys.exit("--seed is solo only: the console attaches to the daemon's sessions, not a resumed one")
    if surface == "solo" and "--seed" not in extra:
        # A session's id picks its badge colour, so a fresh random one made two runs of a scene
        # differ; an empty session under a fixed id is the deterministic start.
        blank = run_dir / "blank.jsonl"
        blank.write_text(json.dumps({"kind": "header", "version": 4, "id": FIXED_ID,
                                     "createdAt": 1790000000000, "cwd": str(Path.cwd())}) + "\n")
        extra += ["--seed", str(blank)]
    if "--seed" in extra:
        at = extra.index("--seed")
        seed = Path(extra[at + 1])
        del extra[at:at + 2]
        meta = json.loads(seed.read_text().splitlines()[0])
        if not re.match(r"^[0-9a-f-]+$", str(meta.get("id", ""))):
            sys.exit(f"--seed {seed}: its header id {meta.get('id')!r} cannot be resumed (scrubbed?)")
        # The store finds a session by the `_<id>.jsonl` suffix of its file name.
        target = (run_dir / "sessions" / encoded(Path.cwd().resolve())
                  / f"{meta['createdAt']}_{meta['id']}.jsonl")
        target.parent.mkdir(parents=True)
        shutil.copy(seed, target)
        extra += ["--session", meta["id"]]
    if surface == "solo":
        argv = [str(binary), "tui", "--headless", "--here", "--model", "faux/faux-1",
                "--session-dir", str(run_dir / "sessions"), *drive, *extra]
        code = subprocess.run(argv, env=env, timeout=600).returncode
    else:
        # A unix socket path is capped near 100 bytes, so it lives under the system temp dir.
        sock = Path(tempfile.mkdtemp(prefix="yi-see.")) / "d.sock"
        log = open(run_dir / "serve.log", "w")
        serve = subprocess.Popen(
            [str(binary), "serve", "--socket", str(sock), "--model", "faux/faux-1",
             "--session-dir", str(run_dir / "sessions")],
            env=env, stdout=subprocess.DEVNULL, stderr=log)
        try:
            for _ in range(200):
                if sock.exists():
                    break
                time.sleep(0.05)
            argv = [str(binary), "console", "--headless", "--socket", str(sock), *drive, *extra]
            code = subprocess.run(argv, env=env, timeout=600).returncode
        finally:
            serve.send_signal(signal.SIGTERM)
            try:
                serve.wait(timeout=10)
            except subprocess.TimeoutExpired:
                serve.kill()
            log.close()
            shutil.rmtree(sock.parent, ignore_errors=True)
    rendered = render(run_dir)
    snaps = sorted(p.stem.removeprefix("snap-") for p in (run_dir / "frames").glob("snap-*.txt"))
    print(f"run: {run_dir} (exit {code})")
    print(f"snaps: {', '.join(snaps) or 'none: add `snap <name>` steps'}")
    if not rendered:
        print("[… pictures skipped: agg and ffmpeg are not both on PATH; the text frames are exact]")
    if code:
        sys.exit(code)
    return run_dir


def render(run_dir: Path) -> bool:
    if not (shutil.which("agg") and shutil.which("ffmpeg")):
        return False
    for cast in (run_dir / "frames").glob("snap-*.cast"):
        gif = cast.with_suffix(".gif")
        subprocess.run(["agg", "--theme", "monokai", "--font-size", "16", str(cast), str(gif)],
                       check=True, capture_output=True)
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(gif), "-frames:v", "1",
                        str(cast.with_suffix(".png"))], check=True)
        gif.unlink()
    return True


SGR_BASIC = {1: ("bold", True), 2: ("dim", True), 3: ("italic", True), 4: ("underline", True),
             5: ("blink", True), 6: ("blink", True), 7: ("reverse", True), 8: ("hidden", True),
             9: ("strike", True), 23: ("italic", False), 24: ("underline", False),
             25: ("blink", False), 27: ("reverse", False), 28: ("hidden", False),
             29: ("strike", False)}


def colour(params: list[int], at: int) -> tuple[str, int]:
    """An extended colour starting at params[at] (38 or 48); returns it and the params used."""
    if params[at + 1:at + 2] == [5] and at + 2 < len(params):
        return f"idx{params[at + 2]}", 3
    if params[at + 1:at + 2] == [2] and at + 4 < len(params):
        r, g, b = params[at + 2:at + 5]
        return f"#{r:02x}{g:02x}{b:02x}", 5
    return "default", 1


def sgr(style: dict, params: list[int]) -> None:
    at = 0
    while at < len(params):
        p = params[at]
        step = 1
        if p == 0:
            style.clear()
        elif p == 22:
            style.pop("bold", None)
            style.pop("dim", None)
        elif p in SGR_BASIC:
            key, value = SGR_BASIC[p]
            if value:
                style[key] = True
            else:
                style.pop(key, None)
        elif 30 <= p <= 37 or 90 <= p <= 97:
            style["fg"] = f"idx{p - 30 if p < 90 else p - 82}"
        elif 40 <= p <= 47 or 100 <= p <= 107:
            style["bg"] = f"idx{p - 40 if p < 100 else p - 92}"
        elif p in (38, 48):
            value, step = colour(params, at)
            style["fg" if p == 38 else "bg"] = value
        elif p == 39:
            style.pop("fg", None)
        elif p == 49:
            style.pop("bg", None)
        at += step


def grid(cast: Path) -> list[list[tuple[str, tuple]]]:
    """The screen a still cast paints: rows of (symbol, style) cells."""
    lines = cast.read_text().splitlines()
    head = json.loads(lines[0])
    cols, rows = head["width"], head["height"]
    cells = [[(" ", ()) for _ in range(cols)] for _ in range(rows)]
    payload = "".join(json.loads(line)[2] for line in lines[1:])
    style: dict = {}
    row = col = 0
    for match in re.finditer(r"\x1b\[([0-9;?]*)([A-Za-z])|\x1b.|(.)", payload, re.S):
        params, final, char = match.group(1), match.group(2), match.group(3)
        if final == "H":
            parts = [int(x) for x in (params or "1;1").split(";") if x] + [1, 1]
            row, col = parts[0] - 1, parts[1] - 1
        elif final == "m":
            sgr(style, [int(x) if x else 0 for x in (params or "0").split(";")])
        elif char is not None and (unicodedata.combining(char) or char in "\u200d\ufe0f"):
            # A mark joins the symbol before it in one cell; the emulator does not advance.
            if 0 <= row < rows and 0 < col <= cols:
                prev, prev_style = cells[row][col - 1]
                cells[row][col - 1] = (prev + char, prev_style)
        elif char is not None and char not in "\r\n":
            wide = unicodedata.east_asian_width(char) in ("W", "F")
            if 0 <= row < rows and 0 <= col < cols:
                cells[row][col] = (char, tuple(sorted(style.items())))
                if wide and col + 1 < cols:
                    cells[row][col + 1] = ("", ())
            col += 2 if wide else 1
    return cells


def screen_rows(txt: Path) -> list[str]:
    """Rows of a frame dump: TestBackend quotes each row and may append the cells a wide
    symbol hides, which are not on screen."""
    return [re.match(r'"(.*)"(?: Hidden by multi-width symbols: .*)?$', line).group(1)
            for line in txt.read_text().splitlines()]


def text_rows(cells) -> list[str]:
    return ["".join(ch for ch, _ in row) for row in cells]


def describe(style: tuple) -> str:
    return " ".join(f"{k}={v}" if v is not True else k for k, v in style) or "plain"


def regions(changed: list[tuple[int, int]]) -> list[tuple[int, int, int, int]]:
    """Boxes (r0, r1, c0, c1) over changed cells, rows within one of each other merged."""
    boxes = []
    for r, c in sorted(changed):
        if boxes and r - boxes[-1][1] <= 1:
            r0, _, c0, c1 = boxes[-1]
            boxes[-1] = (r0, r, min(c0, c), max(c1, c))
        else:
            boxes.append((r, r, c, c))
    return boxes


def png_size(path: Path) -> tuple[int, int]:
    with open(path, "rb") as f:
        return struct.unpack(">II", f.read(24)[16:24])


def crop(png: Path, box, cols: int, rows: int, out: Path) -> str | None:
    width, height = png_size(png)
    cw, ch = width / cols, height / rows
    r0, r1, c0, c1 = box
    x0, x1 = max(0, c0 - MARGIN_COLS), min(cols, c1 + 1 + MARGIN_COLS)
    y0, y1 = max(0, r0 - MARGIN_ROWS), min(rows, r1 + 1 + MARGIN_ROWS)
    w, h = round((x1 - x0) * cw), round((y1 - y0) * ch)
    scale = f",scale={MAX_CROP_PX}:-1" if w > MAX_CROP_PX else ""
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(png), "-vf",
                    f"crop={w}:{h}:{round(x0 * cw)}:{round(y0 * ch)}{scale}", str(out)], check=True)
    return f"[… crop {w} px wide scaled to {MAX_CROP_PX} px (cap {MAX_CROP_PX})]" if scale else None


def diff(a: Path, b: Path) -> Path:
    bundle = b / f"diff-vs-{a.name}"
    shutil.rmtree(bundle, ignore_errors=True)
    bundle.mkdir()
    names_a = {p.stem for p in (a / "frames").glob("snap-*.cast")}
    names_b = {p.stem for p in (b / "frames").glob("snap-*.cast")}
    out = [f"# tui-see diff: {a.name} → {b.name}", ""]
    for missing in sorted(names_a ^ names_b):
        out.append(f"[… snap {missing.removeprefix('snap-')} exists in only one run]")
    for stem in sorted(names_a & names_b):
        before, after = grid(a / "frames" / f"{stem}.cast"), grid(b / "frames" / f"{stem}.cast")
        name = stem.removeprefix("snap-")
        out += [f"## {name}", ""]
        if len(before) != len(after) or len(before[0]) != len(after[0]):
            out += ["screen size differs; compare the whole frames", ""]
            continue
        changed = [(r, c) for r, row in enumerate(after) for c, cell in enumerate(row)
                   if cell != before[r][c]]
        if not changed:
            out += ["no cell changed", ""]
            continue
        out.append(f"{len(changed)} cells changed")
        for r in sorted({r for r, _ in changed}):
            was, now = text_rows([before[r]])[0].rstrip(), text_rows([after[r]])[0].rstrip()
            out.append(f"row {r}: {was!r}" if was == now else f"row {r}: {was!r} → {now!r}")
            runs = {}
            for c in (c for rr, c in changed if rr == r):
                key = (describe(before[r][c][1]), describe(after[r][c][1]))
                if key[0] != key[1]:
                    runs.setdefault(key, []).append(c)
            for (old, new), cs in runs.items():
                spans, start = [], cs[0]
                for prev, col in zip(cs, cs[1:] + [None]):
                    if col != prev + 1:
                        spans.append(f"{start}-{prev}")
                        start = col
                out.append(f"  cols {', '.join(spans)}: {old} → {new}")
        boxes = regions(changed)
        out.append(f"{len(boxes)} changed region(s)")
        if len(boxes) > MAX_REGIONS:
            out.append(f"[… kept {MAX_REGIONS} of {len(boxes)} changed regions (cap {MAX_REGIONS}) "
                       f"— the full frames below hold the rest]")
        png_a, png_b = a / "frames" / f"{stem}.png", b / "frames" / f"{stem}.png"
        if png_a.exists() and png_b.exists():
            rows, cols = len(after), len(after[0])
            for i, box in enumerate(boxes[:MAX_REGIONS]):
                for side, png in (("before", png_a), ("after", png_b)):
                    path = bundle / f"{name}-region{i}-{side}.png"
                    notice = crop(png, box, cols, rows, path)
                    out.append(f"region {i} {side}: {path}" + (f" {notice}" if notice else ""))
            out += [f"whole frame before: {png_a}", f"whole frame after: {png_b}"]
        else:
            out.append("[… no pictures: run with agg and ffmpeg on PATH for crops]")
        out.append("")
    summary = bundle / "summary.md"
    summary.write_text("\n".join(out) + "\n")
    print("\n".join(out))
    return summary


def selfcheck() -> None:
    """Fixtures are real snaps from `yi tui --headless`; their `.txt` comes from the Rust side."""
    runs = [FIXTURES / "empty", FIXTURES / "typed"]
    for run_dir in runs:
        for cast in (run_dir / "frames").glob("snap-*.cast"):
            want = screen_rows(cast.with_suffix(".txt"))
            got = text_rows(grid(cast))
            assert got == want, f"{cast}: parsed text differs from the screen dump"
    # The 16-colour theme draws the reply rail `┃` in cyan, which crossterm writes as index 6.
    answered = grid(runs[1] / "frames" / "snap-answered.cast")
    rails = {describe(style) for row in answered for ch, style in row if ch == "┃"}
    assert rails == {"fg=idx6"}, f"the rail's colour decoded as {rails}"
    after_rail = {describe(row[c + 1][1]) for row in answered for c, (ch, _) in enumerate(row[:-1])
                  if ch == "┃"}
    assert not any("fg=" in style for style in after_rail), "the rail's colour leaked past it"
    with tempfile.TemporaryDirectory() as tmp:
        a, b = Path(tmp) / "empty", Path(tmp) / "typed"
        shutil.copytree(runs[0], a)
        shutil.copytree(runs[1], b)
        summary = diff(a, b).read_text()
        before = screen_rows(a / "frames/snap-composer.txt")
        after = screen_rows(b / "frames/snap-composer.txt")
        rows = {r for r, (x, y) in enumerate(zip(before, after)) if x != y}
        reported = {int(m) for m in re.findall(r"^row (\d+):", summary, re.M)}
        assert rows <= reported, f"text rows {rows - reported} changed but were not reported"
        assert "1 changed region(s)" in summary, "one edited row is one region"
        typed = sum(2 if unicodedata.east_asian_width(c) == "W" else 1
                    for c in "make the rail calmer 易")
        cursor = f"cols {2 + typed}-{2 + typed}: plain → reverse"
        assert cursor in summary, "the cursor's style-only move went unreported"
        assert diff(a, a).read_text().count("no cell changed") == 1
    print("tui_see selfcheck: ok")


def main(argv: list[str]) -> None:
    if argv[:1] == ["--selfcheck"]:
        return selfcheck()
    if argv[:1] == ["diff"] and len(argv) == 3:
        diff(Path(argv[1]).resolve(), Path(argv[2]).resolve())
        return None
    if argv[:1] == ["run"] and len(argv) >= 2:
        opts = dict(zip(argv[2::2], argv[3::2]))
        unknown = set(opts) - {"--label", "--out", "--bin"}
        if unknown or len(argv[2:]) % 2:
            sys.exit(f"run: unknown or valueless flags {sorted(unknown) or argv[-1:]}")
        run(Path(argv[1]), opts.get("--label", "head"), Path(opts.get("--out", "target/see")),
            Path(opts.get("--bin", "target/debug/yi")).resolve())
        return None
    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    main(sys.argv[1:])
