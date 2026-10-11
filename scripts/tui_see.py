#!/usr/bin/env python3
"""Run a TUI scene headless, keep its named snaps, and diff two runs so an agent can see a change.

A scene is a drive script (crates/tui/src/drive.rs grammar) whose `#!` lines carry its setup:
the surface first, then flags, e.g. `#! solo --size 80x24 --faux replies.jsonl --seed s.jsonl`
or `#! console --size 120x34`. `snap <name>` steps mark the frames worth looking at.

  tui_see.py run SCENE [--label head] [--out target/see] [--bin target/debug/yi] [--host term]
  tui_see.py diff RUN_A RUN_B          # two run dirs; writes RUN_B/diff-vs-<A>/summary.md
  tui_see.py --selfcheck

`--host term` also plays the scene in real Ghostty inside the yi-term image (scripts/yi-term),
so the pictures carry kitty images and true colour; it needs `just yi-term` built first.

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


def run(scene: Path, label: str, out: Path, binary: Path, host: str = "mem") -> Path:
    surface, extra = header(scene)
    if not NAME.fullmatch(label):
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
    if host == "term" and not code:
        code = term(scene, run_dir, surface, extra)
    snaps = sorted(p.stem.removeprefix("snap-") for p in (run_dir / "frames").glob("snap-*.txt"))
    print(f"run: {run_dir} (exit {code})")
    print(f"snaps: {', '.join(snaps) or 'none: add `snap <name>` steps'}")
    if not rendered:
        print("[… pictures skipped: agg and ffmpeg are not both on PATH; the text frames are exact]")
    if code:
        sys.exit(code)
    return run_dir


IMAGE = "yi-term:dev"
# The image and the binary follow the host: Ghostty under amd64 emulation took 30 s to map a window.
ARCH = "aarch64" if os.uname().machine in ("arm64", "aarch64") else "x86_64"
LINUX_YI = f"target/{ARCH}-unknown-linux-musl/debug/yi"
SCREEN = "1920x1200"


def term(scene: Path, run_dir: Path, surface: str, extra: list[str]) -> int:
    """Plays the scene in Ghostty in the yi-term container; its grabs replace the agg pictures."""
    linux, host = ROOT / LINUX_YI, ROOT / "target/debug/yi"
    if not linux.exists():
        sys.exit(f"--host term needs {LINUX_YI}: run `just yi-term`")
    if host.exists() and linux.stat().st_mtime < host.stat().st_mtime:
        sys.exit(f"{LINUX_YI} is older than target/debug/yi, so its pictures would show older code: "
                 "run `just yi-term`")
    if not scene.resolve().is_relative_to(ROOT):
        sys.exit(f"--host term mounts the repo, so the scene must live in it: {scene}")
    name = f"yi-term-{os.getpid()}"
    platform = "linux/arm64" if ARCH == "aarch64" else "linux/amd64"
    argv = ["docker", "run", "--rm", "--name", name, "--platform", platform,
            "-v", f"{ROOT}:/repo:ro", "-v", f"{run_dir}:/out", "-w", "/repo", IMAGE,
            "python3", "/repo/scripts/tui_see.py", "term-inner",
            str(scene.resolve().relative_to(ROOT)), surface, *extra]
    try:
        code = subprocess.run(argv, timeout=900).returncode
    except subprocess.TimeoutExpired:
        subprocess.run(["docker", "kill", name], capture_output=True)
        code = 124
    for note in sorted((run_dir / "term").glob("snap-*.note")):
        print(f"[… {note.stem}: {note.read_text()}; the grab may catch a frame mid-draw]")
    pictures = sorted((run_dir / "term").glob("snap-*.png"))
    for png in pictures:
        shutil.copy(png, run_dir / "frames" / png.name)
    if code:
        print(f"[… term host failed (exit {code}); kept {len(pictures)} Ghostty grabs, the rest "
              f"of the pictures are agg's: {run_dir / 'term'}]")
    else:
        print(f"pictures: Ghostty in {IMAGE} ({run_dir / 'term'})")
    return code


NAMED = {"enter": "\r", "return": "\r", "esc": "\x1b", "escape": "\x1b", "tab": "\t",
         "backspace": "\x7f", "space": " "}
# xterm's CSI forms take a modifier parameter; the final byte names the key.
CSI = {"up": "A", "down": "B", "right": "C", "left": "D", "home": "H", "end": "F",
       "f1": "P", "f2": "Q", "f3": "R", "f4": "S"}
TILDE = {"delete": 3, "del": 3, "pageup": 5, "pagedown": 6, "f5": 15, "f6": 17, "f7": 18,
         "f8": 19, "f9": 20, "f10": 21, "f11": 23, "f12": 24}


def key_bytes(spec: str, cmd: bool = False) -> bytes:
    """A drive `key` spec as the bytes a terminal sends: C0 codes and ESC prefixes for plain
    keys, xterm's modifier parameter for named keys, kitty's CSI u for `cmd` (super = 8)."""
    *mods, name = spec.lower().split("-") if spec != "-" else ["-"]
    unknown = set(mods) - {"ctrl", "alt", "shift"}
    if unknown:
        raise ValueError(f"key {spec!r}: unknown modifier {sorted(unknown)}")
    mask = 1 + 1 * ("shift" in mods) + 2 * ("alt" in mods) + 4 * ("ctrl" in mods) + 8 * cmd
    if name in CSI or name in TILDE:
        if name in TILDE:
            return f"\x1b[{TILDE[name]}{f';{mask}' if mask > 1 else ''}~".encode()
        if mask == 1:
            return f"\x1b[{CSI[name]}".encode() if name not in ("f1", "f2", "f3", "f4") \
                else f"\x1bO{CSI[name]}".encode()
        return f"\x1b[1;{mask}{CSI[name]}".encode()
    key = NAMED.get(name, name)
    if len(key) != 1:
        raise ValueError(f"key {spec!r}: no terminal encoding for {name!r}")
    if cmd:
        return f"\x1b[{ord(key)};{mask}u".encode()
    if name == "tab" and "shift" in mods:
        return b"\x1b[Z"
    if "shift" in mods:
        key = key.upper()
    if "ctrl" in mods:
        if key.upper() not in "@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_ " :
            raise ValueError(f"key {spec!r}: ctrl has no C0 code for {key!r}")
        key = chr(ord(key.upper()) & 0x1F)
    return (("\x1b" if "alt" in mods else "") + key).encode()


def term_inner(scene: str, surface: str, extra: list[str]) -> None:
    """Inside the container: Xvfb, then Ghostty running the real binary behind `proxy`."""
    out = Path("/out/term")
    shutil.rmtree(out, ignore_errors=True)
    (out / "home/.yi").mkdir(parents=True)
    (out / "home/.yi/config.json").write_text('{"kernel":{"prewarm":false},"lanes":{"enabled":false}}')
    cols, rows = "80", "24"
    if "--size" in extra:
        at = extra.index("--size")
        cols, rows = extra[at + 1].split("x")
        del extra[at:at + 2]
    if "--session" in extra:
        seed = next(Path("/out/sessions").glob(f"*/*_{extra[extra.index('--session') + 1]}.jsonl"))
        (out / "sessions" / encoded(Path("/repo"))).mkdir(parents=True)
        shutil.copy(seed, out / "sessions" / encoded(Path("/repo")) / seed.name)
    env = {**os.environ, "HOME": str(out / "home"), "DISPLAY": ":99"}
    display = subprocess.Popen(["Xvfb", ":99", "-screen", "0", f"{SCREEN}x24"],
                               stderr=subprocess.DEVNULL)
    if not wait_for(lambda: Path("/tmp/.X11-unix/X99").exists(), 10):
        sys.exit("term: Xvfb did not start")
    yi = f"/repo/{LINUX_YI}"
    serve = None
    if surface == "solo":
        argv = [yi, "tui", "--here", "--model", "faux/faux-1", "--session-dir",
                str(out / "sessions"), *extra]
    else:
        sock = "/tmp/yi-term.sock"
        serve = subprocess.Popen([yi, "serve", "--socket", sock, "--model", "faux/faux-1",
                                  "--session-dir", str(out / "sessions")], env=env,
                                 stderr=open(out / "serve.log", "w"))
        if not wait_for(lambda: Path(sock).exists() or serve.poll() is not None, 10) \
                or serve.poll() is not None:
            sys.exit(f"term: yi serve did not listen; see {out / 'serve.log'}")
        argv = [yi, "console", "--socket", sock, *extra]
    ghostty = subprocess.Popen(
        ["ghostty", "--gtk-single-instance=false", "--window-decoration=false",
         "--font-family=JetBrains Mono", f"--window-width={cols}", f"--window-height={rows}",
         "--background-opacity=1", "--confirm-close-surface=false", "-e", "python3",
         str(ROOT / "scripts/tui_see.py"), "proxy", scene, str(out), "--", *argv],
        env=env, stdout=open(out / "ghostty.log", "w"), stderr=subprocess.STDOUT)
    wait_for(lambda: (out / "done").exists() or ghostty.poll() is not None, 600)
    for proc in (ghostty, serve, display):
        if proc is not None:
            proc.kill()
    done = (out / "done").read_text() if (out / "done").exists() else "no done marker"
    print(f"term: {done}")
    sys.exit(0 if done == "ok" else 1)


def wait_for(ready, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while not ready():
        if time.monotonic() > deadline:
            return False
        time.sleep(0.05)
    return True


def grab(path: Path, box: list[str]) -> None:
    """One keyframe of the Ghostty window: the screen cropped to the box found on the first grab."""
    base = ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
            "-video_size", SCREEN, "-i", ":99"]
    deadline = time.monotonic() + 60
    probe = None
    while not box and time.monotonic() < deadline:
        # cropdetect skips 2 frames by default, and its limit is absolute above 1: the root
        # window is black, Ghostty's background is not.
        probe = subprocess.run(["ffmpeg", "-f", "x11grab", "-draw_mouse", "0", "-video_size",
                                SCREEN, "-i", ":99", "-frames:v", "3", "-vf",
                                "cropdetect=limit=8:round=2:reset=0:skip=0", "-f", "null", "-"],
                               capture_output=True, text=True)
        box += re.findall(r"crop=(\d+:\d+:\d+:\d+)", probe.stderr)[-1:]
        time.sleep(0.5)
    if not box:
        raise RuntimeError("no Ghostty window on the virtual screen within 60 s:\n"
                           + (probe.stderr[-600:] if probe else ""))
    subprocess.run(base + ["-frames:v", "1", "-vf", f"crop={box[0]}", str(path)], check=True)


def proxy(scene: Path, out: Path, argv: list[str]) -> None:
    """Runs inside Ghostty: the binary in a pty, Ghostty's bytes passed both ways, the scene's
    steps sent as key bytes, `wait-frame` read off a pyte screen fed the same bytes, and a grab
    at each `snap` once the output has been quiet."""
    import fcntl
    import pty
    import select
    import termios
    import tty

    import pyte

    pid, fd = pty.fork()
    if pid == 0:
        os.execvp(argv[0], argv)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, fcntl.ioctl(0, termios.TIOCGWINSZ, b"\0" * 8))
    saved = termios.tcgetattr(0)
    tty.setraw(0)
    rows, cols = struct.unpack("HHHH", fcntl.ioctl(0, termios.TIOCGWINSZ, b"\0" * 8))[:2]
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    cast = open(out / "run.cast", "w")
    cast.write(json.dumps({"version": 2, "width": cols, "height": rows}) + "\n")
    began = last = time.monotonic()
    box: list[str] = []
    alive = True

    def pump(seconds: float, until=lambda: False) -> bool:
        nonlocal last, alive
        stop = time.monotonic() + seconds
        while alive and time.monotonic() < stop:
            if until():
                return True
            ready, _, _ = select.select([fd, 0], [], [], 0.02)
            if fd in ready:
                try:
                    data = os.read(fd, 65536)
                except OSError:
                    data = b""
                if not data:
                    alive = False
                    break
                os.write(1, data)
                stream.feed(data)
                cast.write(json.dumps([round(time.monotonic() - began, 6), "o",
                                       data.decode("utf-8", "replace")]) + "\n")
                last = time.monotonic()
            if 0 in ready:
                os.write(fd, os.read(0, 65536))
        return until()

    def shown(needle: str) -> bool:
        return any(needle in row for row in screen.display)

    status = "ok"
    type_ms = 0
    # The binary must be drawing before keys arrive, or the tty's cooked mode eats them.
    if not pump(30, lambda: last > began):
        status = "the binary drew nothing within 30 s"
    for line in scene.read_text().splitlines() if status == "ok" else []:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        word, _, rest = line.partition(" ")
        sent = last
        if word == "key":
            os.write(fd, key_bytes(rest))
            pump(0.05)
        elif word.startswith("cmd-"):
            os.write(fd, key_bytes(word[4:], cmd=True))
            pump(0.05)
        elif word == "type":
            for ch in rest:
                os.write(fd, ch.encode())
                pump(max(type_ms, 5) / 1000)
        elif word == "type-ms":
            type_ms = int(rest)
        elif word == "wait":
            pump(int(rest) / 1000)
        elif word == "wait-idle":
            # The real binary's turn state is not visible here: output that started after the
            # step and then stopped for a second stands in for it.
            if not pump(int(rest) / 1000, lambda: last > sent and time.monotonic() - last >= 1.0):
                status = f"wait-idle {rest} ms timed out"
        elif word == "wait-frame":
            ms, _, needle = rest.partition(" ")
            gone = needle.startswith("!")
            needle = needle[1:] if gone else needle
            if not pump(int(ms) / 1000, lambda: shown(needle) != gone):
                status = f"wait-frame {rest!r} timed out; the screen it saw: {out / 'screen.txt'}"
                (out / "screen.txt").write_text("\n".join(screen.display))
        elif word == "snap":
            if not pump(5, lambda: time.monotonic() - last >= 0.3):
                (out / f"snap-{rest}.note").write_text("output never paused for 0.3 s within 5 s")
            grab(out / f"snap-{rest}.png", box)
        elif word == "quit":
            break
        else:
            status = f"the term host cannot play {word!r} steps"
        if status != "ok" or not alive:
            break
    if not alive and status == "ok":
        status = f"{argv[0]} exited before the scene ended"
    os.kill(pid, signal.SIGTERM)
    termios.tcsetattr(0, termios.TCSADRAIN, saved)
    cast.close()
    (out / "done").write_text(status)


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
    # The bytes a terminal sends: C0 control codes, xterm's CSI Z, kitty's CSI u with super = 8.
    for spec, cmd, want in [("ctrl-c", False, b"\x03"), ("alt-n", False, b"\x1bn"),
                            ("shift-tab", False, b"\x1b[Z"), ("enter", False, b"\r"),
                            ("alt-down", False, b"\x1b[1;3B"), ("ctrl-up", False, b"\x1b[1;5A"),
                            ("f2", False, b"\x1bOQ"), ("pageup", False, b"\x1b[5~"),
                            ("b", True, b"\x1b[98;9u"), ("shift-p", True, b"\x1b[112;10u")]:
        assert key_bytes(spec, cmd) == want, f"{spec}: {key_bytes(spec, cmd)!r}"
    for refused in ("ctrl-1", "hyper-x", "f13"):
        try:
            key_bytes(refused)
        except ValueError:
            continue
        raise AssertionError(f"{refused} was encoded instead of refused")
    print("tui_see selfcheck: ok")


def main(argv: list[str]) -> None:
    if argv[:1] == ["--selfcheck"]:
        return selfcheck()
    if argv[:1] == ["diff"] and len(argv) == 3:
        diff(Path(argv[1]).resolve(), Path(argv[2]).resolve())
        return None
    if argv[:1] == ["term-inner"]:
        return term_inner(argv[1], argv[2], argv[3:])
    if argv[:1] == ["proxy"]:
        out = Path(argv[2])
        try:
            return proxy(Path(argv[1]), out, argv[argv.index("--") + 1:])
        except Exception:
            import traceback
            # Ghostty owns this process's stderr, and its surface closes with the error on it.
            (out / "done").write_text(traceback.format_exc())
            raise
    if argv[:1] == ["run"] and len(argv) >= 2:
        opts = dict(zip(argv[2::2], argv[3::2]))
        unknown = set(opts) - {"--label", "--out", "--bin", "--host"}
        if unknown or len(argv[2:]) % 2:
            sys.exit(f"run: unknown or valueless flags {sorted(unknown) or argv[-1:]}")
        run(Path(argv[1]), opts.get("--label", "head"), Path(opts.get("--out", "target/see")),
            Path(opts.get("--bin", "target/debug/yi")).resolve(), opts.get("--host", "mem"))
        return None
    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    main(sys.argv[1:])
