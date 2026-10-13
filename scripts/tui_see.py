#!/usr/bin/env python3
"""Run a TUI scene headless, keep its named snaps, and diff two runs so an agent can see a change.

A scene is a drive script (crates/tui/src/drive.rs grammar) whose `#!` lines carry its setup:
the surface first, then flags, e.g. `#! solo --size 80x24 --faux replies.jsonl --seed s.jsonl`
or `#! console --size 120x34`. `snap <name>` steps mark the frames worth looking at.

  tui_see.py run SCENE [--label head] [--out target/see] [--bin target/debug/yi] [--host term]
                [--terminal ghostty|kitty|wezterm]
  tui_see.py diff RUN_A RUN_B          # two run dirs; writes RUN_B/diff-vs-<A>/summary.md
  tui_see.py gallery [ROOT] [PAGE]     # every scene and label under ROOT on one HTML page
  tui_see.py watch SCENE [--host term] # base once, then a head run, diff and gallery per save
  tui_see.py terminals SCENE [--out D] # the scene in Ghostty, kitty and WezTerm on one sheet
  tui_see.py harvest SESSION [--name N] [--view end|tool|long] [--size WxH]
                                       # a recorded session as harvest-N-<view> scenes under SCENES
  tui_see.py --selfcheck
  tui_see.py monkey [--runs N] [--seed S] [--surface solo|console]   # random key runs, failures only

`--host term` also plays the scene in real Ghostty inside the yi-term image (scripts/yi-term),
so the pictures carry kitty images and true colour; it needs `just yi-term` built first.
`--terminal kitty|wezterm` plays it in that terminal instead, and each term run prints what the
terminal advertised, the colour forms Yi wrote and the kitty image ids it placed.
`--video` keeps the whole run as motion: run.webp cropped to the Ghostty window on that host,
run.gif rendered by agg on the headless one.
A scene header `--stub <reply file> <tokens/s>` (file beside the scene) swaps the faux model for
scripts/proof/stub_provider.py streaming that reply; such a scene needs `--host term`, and its
`snap-now <name>` step grabs the screen at once, where `snap` waits for output to pause.

What an agent reads, in order: the cell diff as text (exact), then 1:1 crops of each changed
region before and after, then the whole frame. Pictures never gate anything.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))
SCENES = ROOT / "scripts" / "proof" / "tui_see"
sys.path.insert(0, str(ROOT / "evals"))
import record  # noqa: E402  the record-time scrub (§10) harvest must apply before a seed lands
# Loud caps: a cut names what it kept, the cap and the call that gets the rest (045-loud-caps).
MAX_REGIONS = 6
MAX_CROP_PX = 1200
MARGIN_ROWS, MARGIN_COLS = 2, 8


def cap_notice(kept: int, total: int, unit: str, holder: str) -> str:
    """The loud-cap row (045-loud-caps): what was kept of what total, the cap, and where the rest lives."""
    return f"[… kept {kept} of {total} {unit} (cap {kept}); {holder}]"


def header(scene: Path) -> tuple[str, list[str]]:
    words = []
    for line in scene.read_text().splitlines():
        if line.startswith("#!"):
            words += shlex.split(line[2:])
    surface = words.pop(0) if words and words[0] in ("solo", "console") else "solo"
    return surface, words


def take(args: list[str], flag: str, count: int) -> list[str]:
    """Removes `flag` and the `count` words after it from `args`, returning those words."""
    if flag not in args:
        return []
    at = args.index(flag)
    taken = args[at + 1:at + 1 + count]
    del args[at:at + 1 + count]
    return taken


def encoded(cwd: Path) -> str:
    return "--" + str(cwd).strip("/").replace("/", "-").replace(":", "-") + "--"


def scene_config(extra: list[str]) -> str:
    """The run's `config.json`: the two keys every scene needs, then a header's `--config <json>`
    laid over them key by key (a scene that turns a console feature on names it there)."""
    config = {"kernel": {"prewarm": False}, "lanes": {"enabled": False}}
    for patch in take(extra, "--config", 1):
        config.update(json.loads(patch))
    return json.dumps(config, separators=(",", ":"))


NAME = re.compile(r"^[a-z0-9_-]+$")
FIXED_ID = "01a00000-0000-7000-8000-000000000000"


def stage_seed(run_dir: Path, seed: Path) -> str:
    """Copies the seed where the store finds it and keeps a pristine copy; returns its id."""
    meta = json.loads(seed.read_text().splitlines()[0])
    if not re.match(r"^[0-9a-f-]+$", str(meta.get("id", ""))):
        sys.exit(f"--seed {seed}: its header id {meta.get('id')!r} cannot be resumed (scrubbed?)")
    # The store finds a session by the `_<id>.jsonl` suffix of its file name.
    name = f"{meta['createdAt']}_{meta['id']}.jsonl"
    target = run_dir / "sessions" / encoded(Path.cwd().resolve()) / name
    target.parent.mkdir(parents=True)
    shutil.copy(seed, target)
    (run_dir / "seed").mkdir()
    shutil.copy(seed, run_dir / "seed" / name)
    return meta["id"]


def pristine_seed(run_dir: Path, session: str) -> Path:
    """Incident: the Ghostty replay seeded from the session the headless drive had appended to,
    so its pictures showed that run's exchange above the replayed one."""
    return next((run_dir / "seed").glob(f"*_{session}.jsonl"))


def play(scene: Path, surface: str, extra: list[str], run_dir: Path, binary: Path, *,
         record: bool = True, timeout: int = 600,
         stderr=None, stub: bool = False) -> tuple[int, list[str]]:
    """Plays a scene headless in a fresh run dir; returns the exit code and the flags left over."""
    shutil.rmtree(run_dir, ignore_errors=True)
    for sub in ("frames", "sessions", "home/.yi"):
        (run_dir / sub).mkdir(parents=True)
    # Incident: a console scene's session claimed a lane, a `git worktree add` in the shared
    # .git that outlived the run dir and refused the next run as "missing but locked".
    (run_dir / "home/.yi/config.json").write_text(scene_config(extra))
    # ctrl-g hands the draft to $EDITOR; a scene that replays must never open a real vi.
    env = {**os.environ, "HOME": str(run_dir / "home"), "YI_FIXED_IDS": "1",
           "EDITOR": "true", "VISUAL": "true"}
    drive = ["--keys", str(scene.resolve()), "--frames", str(run_dir / "frames"),
             *(["--record", str(run_dir / "run.cast")] if record else [])]
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
        extra += ["--session", stage_seed(run_dir, seed)]
    if stub:
        code = 0
    elif surface == "solo":
        argv = [str(binary), "tui", "--headless", "--here", "--model", "faux/faux-1",
                "--session-dir", str(run_dir / "sessions"), *drive, *extra]
        code = subprocess.run(argv, env=env, timeout=timeout, stderr=stderr).returncode
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
            code = subprocess.run(argv, env=env, timeout=timeout, stderr=stderr).returncode
        finally:
            serve.send_signal(signal.SIGTERM)
            try:
                serve.wait(timeout=10)
            except subprocess.TimeoutExpired:
                serve.kill()
            log.close()
            shutil.rmtree(sock.parent, ignore_errors=True)
    return code, extra


def run(scene: Path, label: str, out: Path, binary: Path, host: str = "mem",
        video: bool = False, terminal: str = "ghostty") -> Path:
    surface, extra = header(scene)
    stub = take(extra, "--stub", 2)
    if stub and host not in ("term", "here"):
        sys.exit("--stub scenes stream from a live provider and are played in Ghostty: add --host term")
    if terminal not in TERMINALS:
        sys.exit(f"--terminal {terminal!r} is not one of {', '.join(TERMINALS)}")
    if not NAME.fullmatch(label):
        sys.exit(f"label {label!r} is not [a-z0-9_-]+")
    run_dir = (out / scene.stem / label).resolve()
    code, extra = play(scene, surface, extra, run_dir, binary, stub=bool(stub))
    rendered = render(run_dir)
    extra += ["--stub", *stub] if stub else []
    extra += ["--terminal", terminal]
    if video and host == "mem":
        gif = run_dir / "run.gif"
        if shutil.which("agg") and agg(run_dir / "run.cast", gif, "--idle-time-limit", "1"):
            print(f"video: {gif} (agg: no kitty images; --host term keeps them)")
        else:
            print("[… --video on the headless host renders with agg, which is not on PATH or "
                  "wrote nothing; no run.gif kept]")
    if host == "term" and not code:
        code = term(scene, run_dir, surface, extra + (["--video"] if video else []))
    if host == "here" and not code:
        code = subprocess.run(["python3", str(ROOT / "scripts/tui_see.py"), "term-inner",
                               str(scene.resolve()), surface, str(run_dir / "term"), str(binary),
                               *extra, *(["--video"] if video else [])], timeout=900).returncode
        collect(run_dir)
    if (run_dir / "term" / "run.cast").exists():
        print(ledger_line(terminal, ledger(run_dir / "term" / "run.cast")))
    snaps = sorted({p.stem.removeprefix("snap-") for p in (run_dir / "frames").glob("snap-*.*")
                    if p.suffix in (".txt", ".png")})
    print(f"run: {run_dir} (exit {code})")
    print(f"snaps: {', '.join(snaps) or 'none: add `snap <name>` steps'}")
    if not rendered:
        print("[… pictures skipped: agg and ffmpeg are not both on PATH; the text frames are exact]")
    if code:
        sys.exit(code)
    return run_dir


# Incident: sessions sharing one Docker daemon each rebuilt yi-term:dev from their own tree, so a
# run got whichever Dockerfile built last; the tag is the content hash `just yi-term` also writes.
IMAGE = "yi-term:" + hashlib.sha256((ROOT / "scripts/yi-term/Dockerfile").read_bytes()).hexdigest()[:12]
# The image and the binary follow the host: Ghostty under amd64 emulation took 30 s to map a window.
ARCH = "aarch64" if os.uname().machine in ("arm64", "aarch64") else "x86_64"
LINUX_YI = f"target/{ARCH}-unknown-linux-musl/debug/yi"
SCREEN = "1920x1200"
TERMINALS = ("ghostty", "kitty", "wezterm")
ITERM = Path("/Applications/iTerm.app")
ADVERTISED = ("TERM", "COLORTERM", "TERM_PROGRAM", "KITTY_WINDOW_ID")
FONT = "Noto Mono"


def terminal_argv(name: str, cols: str, rows: str, cmd: list[str], font: str = FONT,
                  cwd: str | None = None) -> list[str]:
    """One window of `name` running `cmd`. kitty and WezTerm get Ghostty's default ground: the
    grab finds the window by cropdetect against the black root, which their black default hides."""
    launch = {
        "ghostty": ["ghostty", "--gtk-single-instance=false", "--window-decoration=false",
                    f"--font-family={font}", f"--window-width={cols}",
                    f"--window-height={rows}", "--background-opacity=1",
                    "--confirm-close-surface=false", "-e"],
        "kitty": ["kitty", "--config=NONE", "-o", f"font_family={font}", "-o",
                  f"initial_window_width={cols}c", "-o", f"initial_window_height={rows}c", "-o",
                  "remember_window_size=no", "-o", "background=#282c34", "-o",
                  "foreground=#ffffff", "-o", "confirm_os_window_close=0"],
        "wezterm": ["wezterm", "-n", "--config", f"initial_cols={cols}", "--config",
                    f"initial_rows={rows}", "--config", f'font=wezterm.font("{font}")',
                    "--config", "enable_tab_bar=false", "--config", 'window_decorations="NONE"',
                    "--config", 'colors={background="#282c34",foreground="#ffffff"}', "start",
                    "--always-new-process", "--cwd", cwd or os.getcwd(), "--"],
    }
    if name not in launch:
        raise ValueError(f"terminal {name!r} is not one of {', '.join(TERMINALS)}")
    return launch[name] + cmd


def ledger(cast: Path) -> dict:
    """What the terminal advertised (env.json beside the cast) and what Yi wrote to it: the kitty
    image ids it placed, its SGR colour forms, and each OSC number it sent."""
    env = cast.with_name("env.json")
    data = "".join(event[2] for line in cast.read_text().splitlines()[1:]
                   for event in [json.loads(line)] if event[1] == "o")
    placed = set()
    for keys in re.findall(r"\x1b_G([^;\x1b]*)", data):
        fields = dict(kv.split("=", 1) for kv in keys.split(",") if "=" in kv)
        if fields.get("a") in ("p", "T") and fields.get("i", "").isdigit():
            placed.add(int(fields["i"]))
    forms = {"rgb": 0, "256": 0}
    for params in re.findall(r"\x1b\[([0-9;:]*)m", data):
        words, at = re.split("[;:]", params), 0
        while at < len(words):
            kind = words[at + 1] if words[at] in ("38", "48", "58") and at + 1 < len(words) else ""
            forms["rgb"] += kind == "2"
            # crossterm writes the 16-colour palette as indices 0-15 of the 256 form.
            forms["256"] += kind == "5" and at + 2 < len(words) and words[at + 2].isdigit() \
                and int(words[at + 2]) >= 16
            at += {"2": 5, "5": 3}.get(kind, 1)
    tier = "truecolor" if forms["rgb"] else "256" if forms["256"] else "16"
    osc: dict[str, int] = {}
    for number in re.findall(r"\x1b\](\d+);", data):
        osc[number] = osc.get(number, 0) + 1
    return {"env": json.loads(env.read_text()) if env.exists() else {}, "placed": sorted(placed),
            "tier": tier, "forms": forms, "osc": osc}


def families(ids: list[int]) -> str:
    """Kitty image ids by family: solo's orb and logos 76xx, a notebook 77xx, the console's orbs 78xx
    and the rail's avatars 80xx."""
    counts: dict[str, int] = {}
    for image in ids:
        family = f"{image // 100}xx"
        counts[family] = counts.get(family, 0) + 1
    return ", ".join(f"{family} ×{n}" for family, n in counts.items()) or "none"


def ledger_cells(seen: dict) -> list[str]:
    """Colour tier, kitty placements and OSC numbers, as the run line and the sheet both show them."""
    osc = sorted(seen["osc"].items(), key=lambda kv: int(kv[0]))
    return [f"{seen['tier']} (rgb {seen['forms']['rgb']}, 256 {seen['forms']['256']})",
            families(seen["placed"]), ", ".join(f"{n} ×{c}" for n, c in osc) or "none"]


def ledger_line(terminal: str, seen: dict) -> str:
    env = " ".join(f"{k}={v}" for k, v in seen["env"].items() if v is not None) or "env not recorded"
    tier, placed, osc = ledger_cells(seen)
    return f"{terminal}: {env} · colour tier {tier} · kitty placed {placed} · OSC {osc}"


def binary_inputs() -> list[Path]:
    """What the yi binary is built from: crate sources and anything they embed, the manifests,
    the lockfile and the kernel's Python package; tests and benches never reach it."""
    crates = ROOT / "crates"
    return [ROOT / "Cargo.toml", ROOT / "Cargo.lock", *crates.glob("*/Cargo.toml"),
            *crates.glob("*/build.rs"), *crates.glob("*/src/**/*"), *(ROOT / "python").rglob("*")]


def newest_mtime(paths) -> float:
    stamps = []
    for path in paths:
        try:
            stamps.append(path.stat().st_mtime)
        except OSError:
            continue
    return max(stamps, default=0.0)


def words(rows: list[str]) -> list[str]:
    """The words above the solo composer box: the orb's cells, the spinner, wrapping and the
    status row's cwd differ between Ghostty and the headless screen, what was said does not. The
    console has no such row and its key hints follow the terminal, so it yields none."""
    top = next((i for i, row in enumerate(rows) if row.lstrip().startswith("╭")), 0)
    return re.findall(r"\w+", " ".join(rows[:top]))


def drift(run_dir: Path) -> list[str]:
    """Each Ghostty snap's screen, as the proxy's emulator read it, against the headless frame of
    the same name, which played the same scene: where their transcripts part."""
    notes = []
    for screen in sorted((run_dir / "term").glob("snap-*.screen")):
        frame = run_dir / "frames" / f"{screen.stem}.txt"
        if not frame.exists():
            continue
        want, got = words(screen_rows(frame)), words(screen.read_text().split("\n"))
        shown = min(len(want), len(got))
        # The orb's rows push more of Ghostty's transcript into scrollback: the tails must agree.
        if want[len(want) - shown:] != got[len(got) - shown:]:
            notes.append(f"[… snap {screen.stem.removeprefix('snap-')}: Ghostty's transcript ends "
                         f"{' '.join(got[-6:])!r}, the headless one {' '.join(want[-6:])!r}; "
                         f"compare {screen} with {frame}]")
    return notes


def collect(run_dir: Path) -> list[Path]:
    """Ghostty's grabs replace agg's pictures in frames/; its notes, text drift and video are
    surfaced."""
    for note in sorted((run_dir / "term").glob("*.note")):
        print(f"[… {note.stem}: {note.read_text()}; the grab may catch a frame mid-draw]")
    for note in drift(run_dir):
        print(note)
    if (run_dir / "term" / "run.webp").exists():
        shutil.copy(run_dir / "term" / "run.webp", run_dir / "run.webp")
        print(f"video: {run_dir / 'run.webp'}")
    pictures = sorted((run_dir / "term").glob("snap-*.png"))
    for png in pictures:
        shutil.copy(png, run_dir / "frames" / png.name)
    return pictures


def term(scene: Path, run_dir: Path, surface: str, extra: list[str]) -> int:
    """Plays the scene in Ghostty in the yi-term container; its grabs replace the agg pictures."""
    linux = ROOT / LINUX_YI
    if not linux.exists():
        sys.exit(f"--host term needs {LINUX_YI}: run `just yi-term`")
    # Against the sources, not the host binary: cargo leaves an up-to-date artifact's mtime alone.
    if linux.stat().st_mtime < newest_mtime(binary_inputs()):
        sys.exit(f"{LINUX_YI} is older than the sources, so its pictures would show older code: "
                 "run `just yi-term`")
    if not scene.resolve().is_relative_to(ROOT):
        sys.exit(f"--host term mounts the repo, so the scene must live in it: {scene}")
    found = subprocess.run(["docker", "image", "inspect", IMAGE], capture_output=True, text=True)
    if found.returncode:
        sys.exit(f"--host term needs the image {IMAGE} built from this tree (`just yi-term`); "
                 f"docker said: {found.stderr.strip()[-300:]}")
    name = f"yi-term-{os.getpid()}"
    platform = "linux/arm64" if ARCH == "aarch64" else "linux/amd64"
    argv = ["docker", "run", "--rm", "--name", name, "--platform", platform,
            "-v", f"{ROOT}:/repo:ro", "-v", f"{run_dir}:/out", "-w", "/repo", IMAGE,
            "python3", "/repo/scripts/tui_see.py", "term-inner",
            str(scene.resolve().relative_to(ROOT)), surface, "/out/term", f"/repo/{LINUX_YI}", *extra]
    try:
        code = subprocess.run(argv, timeout=900).returncode
    except subprocess.TimeoutExpired:
        subprocess.run(["docker", "kill", name], capture_output=True)
        code = 124
    pictures = collect(run_dir)
    if code:
        print(f"[… term host failed (exit {code}); kept {len(pictures)} terminal grabs, the rest "
              f"of the pictures are agg's: {run_dir / 'term'}]")
    else:
        print(f"pictures: {(run_dir / 'term' / 'terminal').read_text()} in {IMAGE} ({run_dir / 'term'})")
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


STUB_MODEL = "openrouter/local-slow"
PROXY_VARS = ("HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "https_proxy", "http_proxy", "all_proxy")


def start_stub(reply: Path, rate: str, home: Path, env: dict[str, str]) -> subprocess.Popen:
    """Streams `reply` at `rate` tokens a second from a local OpenAI-style endpoint, registered in
    `home`'s catalog as STUB_MODEL, and points `env` at it with no proxy in the way."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    stub = subprocess.Popen([sys.executable, str(Path(__file__).with_name("proof") / "stub_provider.py"),
                             str(port), str(reply), rate], stderr=open(home.parent / "stub.log", "w"))
    model = {"id": "local-slow", "name": "Local Slow", "api": "openai-completions",
             "provider": "openrouter", "baseUrl": f"http://127.0.0.1:{port}/v1", "reasoning": False,
             "input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
             "contextWindow": 128000, "maxTokens": 8000}
    (home / ".yi/catalog").mkdir(parents=True)
    (home / ".yi/catalog/openrouter.json").write_text(
        json.dumps({"openai-completions": {"local-slow": model}}))
    env.update(OPENROUTER_API_KEY="stub", NO_PROXY="*", no_proxy="*")
    for var in PROXY_VARS:
        env.pop(var, None)

    def listening() -> bool:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return True
        except OSError:
            return False
    if not wait_for(listening, 10):
        sys.exit("term: the stub provider did not listen")
    return stub


def term_inner(scene: str, surface: str, out: Path, yi: str, extra: list[str]) -> None:
    """Inside the yi-term image: Xvfb, then the terminal running the real binary behind `proxy`."""
    shutil.rmtree(out, ignore_errors=True)
    (out / "home/.yi").mkdir(parents=True)
    terminal = (take(extra, "--terminal", 1) or ["ghostty"])[0]
    (out / "terminal").write_text(terminal)
    (out / "home/.yi/config.json").write_text(scene_config(extra))
    video = "--video" in extra
    extra = [arg for arg in extra if arg != "--video"]
    cols, rows = "80", "24"
    if "--size" in extra:
        at = extra.index("--size")
        cols, rows = extra[at + 1].split("x")
        del extra[at:at + 2]
    stub = take(extra, "--stub", 2)
    # Incident: a turn's checkpoint snapshots the cwd, and in the mounted repo (gigabytes of
    # target/) a faux turn took 20 s to minutes to go idle, so an exact `wait-idle` timed out.
    work = Path("/tmp/yi-scene")
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir()
    scene = str(Path(scene).resolve())
    if "--faux" in extra:
        at = extra.index("--faux") + 1
        extra[at] = str(Path(extra[at]).resolve())
    if "--session" in extra:
        seed = pristine_seed(out.parent, extra[extra.index("--session") + 1])
        (out / "sessions" / encoded(work)).mkdir(parents=True)
        shutil.copy(seed, out / "sessions" / encoded(work) / seed.name)
    # One job plays many scenes: a server that exited leaves its lock, so each run takes a free one.
    number = next(n for n in range(99, 400) if not Path(f"/tmp/.X{n}-lock").exists())
    # Incident: kitty's image cache made its temp files on the host-mounted /out and failed every
    # defrag; a cache on the container's own disk keeps its images.
    env = {**os.environ, "HOME": str(out / "home"), "DISPLAY": f":{number}", "YI_FIXED_IDS": "1",
           "EDITOR": "true", "VISUAL": "true",
           "XDG_CACHE_HOME": tempfile.mkdtemp(prefix="yi-term-cache.")}
    display = subprocess.Popen(["Xvfb", f":{number}", "-screen", "0", f"{SCREEN}x24"],
                               stderr=subprocess.DEVNULL)
    procs = [display]
    model = STUB_MODEL if stub else "faux/faux-1"
    try:
        if not wait_for(lambda: Path(f"/tmp/.X11-unix/X{number}").exists(), 10):
            sys.exit("term: Xvfb did not start")
        if stub:
            procs.append(start_stub(Path(scene).parent / stub[0], stub[1], out / "home", env))
        if surface == "solo":
            argv = [yi, "tui", "--here", "--model", model, "--session-dir",
                    str(out / "sessions"), *extra]
        else:
            # Incident: the `here` host plays base and head in one container, and the killed base
            # serve left a shared socket the head console reached before the new serve listened.
            sock = str(Path(tempfile.mkdtemp(prefix="yi-term.")) / "d.sock")
            serve = subprocess.Popen([yi, "serve", "--socket", sock, "--model", model,
                                      "--session-dir", str(out / "sessions")], env=env, cwd=work,
                                     stderr=open(out / "serve.log", "w"))
            procs.append(serve)
            if not wait_for(lambda: Path(sock).exists() or serve.poll() is not None, 10) \
                    or serve.poll() is not None:
                sys.exit(f"term: yi serve did not listen; see {out / 'serve.log'}")
            argv = [yi, "console", "--socket", sock, *extra]
        recorder = None
        if video:
            recorder = subprocess.Popen(
                ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
                 "-framerate", "10", "-video_size", SCREEN, "-i", env["DISPLAY"], "-c:v", "libx264",
                 "-preset", "ultrafast", "-pix_fmt", "yuv420p", str(out / "screen.mp4")],
                stdin=subprocess.PIPE)
            procs.append(recorder)
        window = subprocess.Popen(
            terminal_argv(terminal, cols, rows, ["python3", str(ROOT / "scripts/tui_see.py"),
                                                 "proxy", scene, str(out), "--", *argv],
                          cwd=str(work)),
            env=env, cwd=work, stdout=open(out / f"{terminal}.log", "w"), stderr=subprocess.STDOUT)
        procs.append(window)
        wait_for(lambda: (out / "done").exists() or window.poll() is not None, 600)
        if recorder is not None:
            try:
                recorder.communicate(b"q", timeout=30)
                encode_video(out)
            except (subprocess.SubprocessError, OSError) as error:
                (out / "video.note").write_text(f"the run was not kept as video: {error}")
    finally:
        for proc in reversed(procs):
            proc.kill()
            proc.wait()
    done = (out / "done").read_text() if (out / "done").exists() else "no done marker"
    print(f"term: {done}")
    sys.exit(0 if done == "ok" else 1)


def encode_video(out: Path) -> None:
    """The recording cropped to the window the first grab found, as an animated WebP small
    enough for a README (the repo caps a committed file at 512 KB)."""
    box = out / "box"
    if not box.exists():
        (out / "video.note").write_text("no snap step ran, so no window box was found to crop to")
        return
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(out / "screen.mp4"), "-vf",
                    f"crop={box.read_text()},fps=6,scale=1064:-1:flags=lanczos", "-c:v",
                    "libwebp_anim", "-q:v", "50", "-loop", "0", str(out / "run.webp")],
                   check=True)


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
            "-video_size", SCREEN, "-i", os.environ.get("DISPLAY", ":99")]
    deadline = time.monotonic() + 60
    probe = None
    while not box and time.monotonic() < deadline:
        # cropdetect skips 2 frames by default, and its limit is absolute above 1: the root
        # window is black, Ghostty's background is not.
        probe = subprocess.run(["ffmpeg", "-f", "x11grab", "-draw_mouse", "0", "-video_size",
                                SCREEN, "-i", os.environ.get("DISPLAY", ":99"), "-frames:v", "3", "-vf",
                                "cropdetect=limit=8:round=2:reset=0:skip=0", "-f", "null", "-"],
                               capture_output=True, text=True)
        box += re.findall(r"crop=(\d+:\d+:\d+:\d+)", probe.stderr)[-1:]
        time.sleep(0.5)
    if not box:
        raise RuntimeError("no terminal window on the virtual screen within 60 s:\n"
                           + (probe.stderr[-600:] if probe else ""))
    (path.parent / "box").write_text(box[0])
    subprocess.run(base + ["-frames:v", "1", "-vf", f"crop={box[0]}", str(path)], check=True)


class ApcFilter:
    """Drops APC sequences (ESC _ … ESC \\) from a byte stream, holding one split across reads:
    pyte prints a kitty image's base64 payload as text, which buried the rows a wait looks for."""

    def __init__(self) -> None:
        self.held = b""

    def feed(self, data: bytes) -> bytes:
        data = re.sub(rb"\x1b_.*?\x1b\\", b"", self.held + data, flags=re.S)
        start = data.find(b"\x1b_")
        self.held, data = (data[start:], data[:start]) if start >= 0 else (b"", data)
        if data.endswith(b"\x1b"):
            self.held, data = b"\x1b" + self.held, data[:-1]
        if len(self.held) > 1 << 16:
            # An image is far smaller; an APC that never ends must not swallow the screen.
            data, self.held = data + self.held, b""
        return data


TIMING = ("tick", "hold", "release", "wait-idle")


def steps(scene: Path) -> list[str]:
    """The scene's step lines as the proxy plays them: `snap-strip` spelled out as the drive
    parses it, and a frozen scene opened with `tick 0`, as any tick freezes a headless run."""
    lines = [line.strip() for line in scene.read_text().splitlines()]
    lines = [line for line in lines if line and not line.startswith("#")]
    played = ["tick 0"] if any(line.split(" ", 1)[0] in ("tick", "snap-strip") for line in lines) else []
    for line in lines:
        word, _, rest = line.partition(" ")
        if word != "snap-strip":
            played.append(line)
            continue
        name, count, ms = rest.split()
        for frame in range(int(count)):
            if frame:
                played.append(f"tick {ms}")
            played.append(f"snap {name}-{frame:02}")
    return played


def proxy(scene: Path, out: Path, argv: list[str]) -> None:
    """Runs inside the terminal: the binary in a pty, its bytes passed both ways, the scene's
    keys sent as key bytes and its timing steps over the binary's `--control` socket, which
    answers each once played, `wait-frame` read off a pyte screen fed the same bytes, and a grab
    at each `snap` once the output has been quiet."""
    import fcntl
    import pty
    import select
    import socket
    import termios
    import tty

    import pyte

    (out / "env.json").write_text(json.dumps({key: os.environ.get(key) for key in ADVERTISED}))
    # A unix socket path is capped near 100 bytes, so it lives under the system temp dir.
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(Path(tempfile.mkdtemp(prefix="yi-ctl.")) / "c.sock"))
    listener.listen(1)
    listener.settimeout(30)
    argv = [*argv, "--control", listener.getsockname()]
    extra = header(scene)[1]
    asked = extra[extra.index("--size") + 1] if "--size" in extra else "80x24"
    # Incident: the terminal starts this process at a smaller grid and then grows to the one
    # asked for; a child sized before that drew an 80x18 scene at 79x17.
    if not wait_for(lambda: "{1}x{0}".format(*struct.unpack(
            "HHHH", fcntl.ioctl(0, termios.TIOCGWINSZ, b"\0" * 8))[:2]) == asked, 10):
        (out / "size.note").write_text(f"The terminal never reached the scene's {asked} grid")
    pid, fd = pty.fork()
    if pid == 0:
        os.execvp(argv[0], argv)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, fcntl.ioctl(0, termios.TIOCGWINSZ, b"\0" * 8))
    saved = termios.tcgetattr(0)
    tty.setraw(0)
    rows, cols = struct.unpack("HHHH", fcntl.ioctl(0, termios.TIOCGWINSZ, b"\0" * 8))[:2]
    class Screen(pyte.Screen):
        """Incident: pyte has no SU or SD (CSI S, CSI T), which the inline viewport scrolls its
        region with, so after the first commit the rows a wait read were not Ghostty's."""

        def scroll(self, count, down):
            top, bottom = self.margins or pyte.screens.Margins(0, self.lines - 1)
            rows = list(range(top, bottom + 1))
            for _ in range(count or 1):
                old = [self.buffer[y] for y in rows]
                moved = zip(rows[1:], old[:-1]) if down else zip(rows[:-1], old[1:])
                for y, line in moved:
                    self.buffer[y] = line
                self.buffer.pop(top if down else bottom, None)
            self.dirty.update(range(self.lines))

        def scroll_up(self, count=1, *_, **__):
            self.scroll(count, False)

        def scroll_down(self, count=1, *_, **__):
            self.scroll(count, True)

    class Stream(pyte.ByteStream):
        csi = {**pyte.ByteStream.csi, "S": "scroll_up", "T": "scroll_down"}

    screen = Screen(cols, rows)
    stream = Stream(screen)
    apc = ApcFilter()
    cast = open(out / "run.cast", "w")
    cast.write(json.dumps({"version": 2, "width": cols, "height": rows}) + "\n")
    began = last = time.monotonic()
    box: list[str] = []
    alive = True
    control = None
    answer = b""

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
                stream.feed(apc.feed(data))
                cast.write(json.dumps([round(time.monotonic() - began, 6), "o",
                                       data.decode("utf-8", "replace")]) + "\n")
                last = time.monotonic()
            if 0 in ready:
                os.write(fd, os.read(0, 65536))
        return until()

    def shown(needle: str) -> bool:
        return any(needle in row for row in screen.display)

    def answered() -> bool:
        nonlocal answer
        try:
            answer += control.recv(4096)
        except BlockingIOError:
            pass
        return answer.endswith(b"\n")

    status = "ok"
    type_ms = 0
    # The binary must be drawing before keys arrive, or the tty's cooked mode eats them.
    if not pump(30, lambda: last > began):
        status = "the binary drew nothing within 30 s"
    else:
        # The binary connects before it takes the screen, so the connection is already queued.
        control = listener.accept()[0]
        control.setblocking(False)
    for line in steps(scene) if status == "ok" else []:
        word, _, rest = line.partition(" ")
        if word in TIMING:
            answer = b""
            control.sendall(line.encode() + b"\n")
            # The binary times a wait out itself; this bound is for a binary that stopped answering.
            limit = (int(rest) if word == "wait-idle" else 0) / 1000 + 30
            if not pump(limit, answered):
                status = f"{line}: no answer from the binary within {limit:.0f} s"
            elif answer != b"ok\n":
                status = f"{line}: {answer.decode(errors='replace').strip()}"
                (out / "screen.txt").write_text("\n".join(screen.display))
        elif word == "key":
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
        elif word == "wait-frame":
            ms, _, needle = rest.partition(" ")
            gone = needle.startswith("!")
            needle = needle[1:] if gone else needle
            if not pump(int(ms) / 1000, lambda: shown(needle) != gone):
                status = f"wait-frame {rest!r} timed out; the screen it saw: {out / 'screen.txt'}"
                (out / "screen.txt").write_text("\n".join(screen.display))
        elif word == "snap-now":
            grab(out / f"snap-{rest}.png", box)
        elif word == "snap":
            if not pump(5, lambda: time.monotonic() - last >= 0.3):
                (out / f"snap-{rest}.note").write_text("output never paused for 0.3 s within 5 s")
            grab(out / f"snap-{rest}.png", box)
            (out / f"snap-{rest}.screen").write_text("\n".join(screen.display))
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


def agg(cast: Path, gif: Path, *flags: str) -> bool:
    """One pinned renderer for stills and motion, so every proof looks alike across machines.
    Incident: agg can exit 0 and write an empty file, so a render checks its bytes."""
    done = subprocess.run(["agg", "--theme", "monokai", "--font-size", "16", *flags, str(cast),
                           str(gif)], capture_output=True)
    return done.returncode == 0 and gif.exists() and gif.stat().st_size > 0


def render(run_dir: Path) -> bool:
    if not (shutil.which("agg") and shutil.which("ffmpeg")):
        return False
    for cast in (run_dir / "frames").glob("snap-*.cast"):
        gif = cast.with_suffix(".gif")
        if not agg(cast, gif):
            raise subprocess.CalledProcessError(1, ["agg", str(cast)])
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
            out.append(cap_notice(MAX_REGIONS, len(boxes), "changed regions", "the full frames below hold the rest"))
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


def labels(scene: Path) -> list[Path]:
    runs = [p for p in scene.iterdir() if (p / "frames").is_dir()]
    order = {"base": 0, "head": 1}
    return sorted(runs, key=lambda p: (order.get(p.name, 2), p.name))


PAGE_CAP = 15_000_000


def host_name(run_dir: Path, stem: str) -> str:
    named = run_dir / "term" / "terminal"
    if not (run_dir / "term" / f"{stem}.png").exists():
        return "agg"
    return named.read_text() if named.exists() else "ghostty"


def gallery(root: Path, page: Path, videos_kept: bool = True, preface: str = "") -> Path:
    """Every scene under `root`, each snap with its labels side by side and the cell diff a head
    run keeps against its base: one self-contained page (pictures inlined)."""
    import base64
    import html

    def picture(run_dir: Path, stem: str) -> str:
        png = run_dir / "frames" / f"{stem}.png"
        if png.exists():
            data = base64.b64encode(png.read_bytes()).decode()
            return f'<img alt="{html.escape(run_dir.name)} {stem}" src="data:image/png;base64,{data}">'
        txt = run_dir / "frames" / f"{stem}.txt"
        return f"<pre>{html.escape(chr(10).join(screen_rows(txt)))}</pre>"

    def caption(run_dir: Path, stem: str) -> str:
        head = json.loads((run_dir / "frames" / f"{stem}.cast").read_text().splitlines()[0])
        host = host_name(run_dir, stem)
        return f"{html.escape(run_dir.name)} · {head['width']}×{head['height']} · {host}"

    sections, skipped = [], []
    for scene in sorted(p for p in root.iterdir() if p.is_dir()):
        runs = labels(scene)
        stems = sorted({p.stem for run_dir in runs for p in (run_dir / "frames").glob("snap-*.txt")})
        if not stems:
            skipped.append(scene.name)
            continue
        diffs = {f"{p.parent.parent.name} {p.parent.name}": p.read_text()
                 for p in scene.glob("*/diff-vs-*/summary.md")}
        cards = []
        for stem in stems:
            name = stem.removeprefix("snap-")
            figures = "".join(
                f"<figure><figcaption>{caption(r, stem)}</figcaption>{picture(r, stem)}</figure>"
                for r in runs if (r / "frames" / f"{stem}.txt").exists())
            notes = "".join(f"<p class=note>{html.escape(n.read_text())}</p>"
                            for r in runs for n in (r / "term").glob(f"{stem}.note")
                            if (r / "term").is_dir())
            changed = "".join(
                f"<pre class=diff>{html.escape(label)}\n{html.escape(m.group(1).strip())}</pre>"
                for label, text in diffs.items()
                for m in [re.search(rf"^## {re.escape(name)}\n(.*?)(?=^## |\Z)", text, re.M | re.S)]
                if m)
            cards.append(f"<article><h3>{html.escape(name)}</h3>{notes}{changed}"
                         f"<div class=row>{figures}</div></article>")
        videos = "".join(
            f"<figure><figcaption>{html.escape(r.name)} · whole run</figcaption><img "
            f'alt="{html.escape(r.name)} run" src="data:image/webp;base64,'
            f'{base64.b64encode((r / "run.webp").read_bytes()).decode()}"></figure>'
            for r in runs if videos_kept and (r / "run.webp").exists())
        if videos:
            cards.insert(0, f"<article><h3>run</h3><div class=row>{videos}</div></article>")
        sections.append(f"<section><h2>{html.escape(scene.name)}</h2>{''.join(cards)}</section>")
    body = GALLERY.replace("{sections}", preface + ("".join(sections) or "<p>No snaps yet.</p>"))
    if len(body) > PAGE_CAP and videos_kept:
        print(f"[… {len(body) / 1e6:.1f} MB is past the {PAGE_CAP / 1e6:.0f} MB page cap; "
              "dropping the run videos, each still at <scene>/<label>/run.webp]")
        return gallery(root, page, videos_kept=False, preface=preface)
    page.write_text(body)
    if skipped:
        print(f"[… {len(skipped)} scene(s) have no snaps and are not on the page: {', '.join(skipped)}]")
    if len(body) > PAGE_CAP:
        print(f"[… the page is {len(body) / 1e6:.1f} MB, past the {PAGE_CAP / 1e6:.0f} MB page cap "
              "(PAGE_CAP); gallery a narrower root for a smaller one]")
    print(f"gallery: {page} ({len(body) / 1e6:.1f} MB)")
    return page


GALLERY = """<title>Yi TUI gallery</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;600&family=IBM+Plex+Sans:wght@400;600&display=swap">
<style>
/* A contact sheet: one band per scene, each snap's labels side by side, the cell diff above. */
:root{--bg:#f4f5f7;--fg:#1b1f27;--muted:#5b6474;--sheet:#ffffff;--rule:#d8dce4;--accent:#3d5afe;
--warn:#b54708;--ui:"IBM Plex Sans",system-ui,sans-serif;--mono:"JetBrains Mono",ui-monospace,Menlo,monospace}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#13161c;--fg:#e3e6ec;
--muted:#99a2b3;--sheet:#1c2028;--rule:#2f3542;--accent:#8c9eff;--warn:#f79009;color-scheme:dark}}
:root[data-theme="dark"]{--bg:#13161c;--fg:#e3e6ec;--muted:#99a2b3;--sheet:#1c2028;--rule:#2f3542;
--accent:#8c9eff;--warn:#f79009;color-scheme:dark}
body{background:var(--bg);color:var(--fg);font:15px/1.5 var(--ui);padding-block:20px 40px;
padding-inline:16px;max-width:1280px;margin-inline:auto}
h1{font-size:22px;margin:0;text-wrap:balance}h2{font:600 13px/1 var(--mono);letter-spacing:.06em;
text-transform:uppercase;color:var(--accent);margin:32px 0 12px}h3{font:600 14px/1.3 var(--mono);margin:0}
.lede{color:var(--muted);margin:6px 0 0;max-width:65ch}
article{display:grid;gap:10px;background:var(--sheet);border:1px solid var(--rule);border-radius:8px;
padding:14px;margin:0 0 14px}.row{display:flex;flex-wrap:wrap;gap:14px}
figure{margin:0;flex:1 1 340px;min-width:0;display:grid;gap:6px}
figcaption{font:12px/1.3 var(--mono);color:var(--muted);font-variant-numeric:tabular-nums}
img{max-width:100%;height:auto;border-radius:4px;display:block}
pre{margin:0;font:12px/1.45 var(--mono);white-space:pre-wrap;overflow-wrap:anywhere;color:var(--fg)}
pre.diff{color:var(--muted);border-left:2px solid var(--rule);padding-left:10px}
.note{color:var(--warn);font-size:13px;margin:0}.wide{overflow-x:auto}
table{border-collapse:collapse;font:12px/1.4 var(--mono)}th,td{text-align:left;vertical-align:top;
padding:4px 10px;border-bottom:1px solid var(--rule)}code{font-family:var(--mono);font-size:.92em}
</style>
<h1>Yi TUI gallery</h1>
<p class="lede">Snaps from <code>scripts/tui_see.py</code>: each scene's labels side by side, with the cell
diff a head run keeps against its base. Comment on any snap to send a note back.</p>
{sections}
"""


def watch(scene: Path, host: str) -> None:
    """Runs the scene once as `base`, then again as `head` after every save under the TUI crates
    or to the scene, each time diffing against base and rewriting the gallery."""
    out = Path("target/see")
    sources = [ROOT / "crates" / name / "src" for name in ("tui", "console", "orb")]
    build = [["cargo", "build", "-q", "-p", "yi-cli"]] + ([["just", "yi-term"]] if host == "term" else [])
    binary = (ROOT / "target/debug/yi").resolve()

    def stamp() -> float:
        return newest_mtime([scene, *(f for src in sources for f in src.rglob("*.rs"))])

    def attempt(label: str) -> Path | None:
        try:
            if any(subprocess.run(step).returncode for step in build):
                print("[… build failed; waiting for the next save]")
                return None
            return run(scene, label, out, binary, host)
        except SystemExit as stop:
            print(f"[… the {label} run failed ({stop.code}); waiting for the next save]")
        except (subprocess.SubprocessError, OSError) as error:
            print(f"[… the {label} run could not start ({error}); waiting for the next save]")
        return None

    # A base kept from an older scene or another host would diff against the wrong thing.
    base = out / scene.stem / "base"
    if attempt("base") is None:
        sys.exit("watch: the base run failed; fix it before watching")
    seen = stamp()
    print(f"watching {scene} (ctrl-c stops)")
    while True:
        time.sleep(1)
        if stamp() == seen:
            continue
        seen = stamp()
        head = attempt("head")
        if head is not None:
            diff(base, head)
            gallery(out, out / "gallery.html")


def iterm_notice(installed: bool) -> str:
    """iTerm2 is a terminal Yi claims that the Linux image cannot run, so the sheet says so."""
    where = f"installed at {ITERM} but not driven" if installed else f"not installed at {ITERM}"
    return (f"[… kept {len(TERMINALS)} of {len(TERMINALS) + 1} terminals (cap: the yi-term image "
            f"runs Linux terminals only); iTerm2 is macOS-only and {where}, so play the scene in "
            "it by hand]")


def terminals(scene: Path, out: Path, binary: Path) -> Path:
    """The scene in each terminal of the yi-term image, on one sheet: the grabs side by side
    under a table of what each terminal advertised and what Yi placed and coloured there."""
    import html
    root = (out / scene.stem).resolve()
    rows, failed = [], []
    for name in TERMINALS:
        try:
            run(scene, name, root, binary, "term", terminal=name)
            verdict = "ok"
        except SystemExit as stop:
            verdict = f"failed (exit {stop.code})"
            failed.append(f"{name} {verdict}")
        cast = root / scene.stem / name / "term" / "run.cast"
        seen = ledger(cast) if cast.exists() else None
        cells = [name, *(seen["env"].get(k) or "unset" for k in ADVERTISED), *ledger_cells(seen)] \
            if seen else [name, *["—"] * (len(ADVERTISED) + 3)]
        rows.append("<tr>" + "".join(f"<td>{html.escape(c)}</td>" for c in [*cells, verdict]) + "</tr>")
    notice = iterm_notice(ITERM.exists())
    print(notice)
    head = "".join(f"<th>{h}</th>" for h in ("terminal", *ADVERTISED, "colour tier", "kitty placed",
                                              "OSC", "run"))
    preface = (f"<section><h2>terminals</h2><div class=wide><table><tr>{head}</tr>{''.join(rows)}"
               f"</table></div><p class=note>{html.escape(notice)}</p></section>")
    page = gallery(root, root / "terminals.html", preface=preface)
    if failed:
        sys.exit(f"terminals: {'; '.join(failed)}; the sheet still shows what each run kept")
    return page


MASK = "[MASKED]"
VIEWS = ("end", "tool", "long")


def resumable(rows: list[dict]) -> list[dict]:
    """The scrub (evals/record.py) masks the header id and most entry ids, which `--seed` refuses:
    duplicate ids, then a parent that is not the lane leaf. Masked ids become the row's seq in
    hex and masked parents the previous entry; the header id is derived from the content."""
    digest = hashlib.sha256(json.dumps(rows).encode()).hexdigest()
    if rows[0].get("id") == MASK:
        rows[0]["id"] = f"01a0{digest[:4]}-{digest[4:8]}-7{digest[8:11]}-8{digest[11:14]}-{digest[14:26]}"
    leaf = None
    for row in rows[1:]:
        if row.get("id") == MASK:
            row["id"] = f"{row['seq']:08x}"
        if row.get("parentId") == MASK:
            row["parentId"] = leaf
        if row.get("kind") == "entry":
            leaf = row["id"]
    return rows


def text_of(message: dict) -> str:
    content = message["content"]
    if isinstance(content, str):
        return content
    return "".join(block.get("text", "") for block in content if block.get("type") == "text")


def cut(rows: list[dict], view: str) -> int | None:
    """How many rows a view keeps: all of them, through the largest tool result, or through the
    longest reply; the last two run on past their pick to the next turn so no call is left open.
    None when the session has no row of the view's kind."""
    picks = [(i, row["message"]) for i, row in enumerate(rows) if row.get("type") == "message"]
    if view == "end":
        return len(rows)
    if view == "tool":
        candidates = [i for i, m in picks if m["role"] == "toolResult"]
    else:
        candidates = [i for i, m in picks if m["role"] == "assistant"]
    if not candidates:
        return None
    key = (lambda i: len(json.dumps(rows[i]))) if view == "tool" \
        else (lambda i: len(text_of(rows[i]["message"])))
    pick = max(candidates, key=key)
    end = pick + 1
    while end < len(rows) and (rows[end].get("message") or {}).get("role") not in ("user", "assistant"):
        end += 1
    return end


def continuation(rows: list[dict]) -> tuple[str, dict, str] | None:
    """The last typed prompt and the last reply that has text, as the faux cassette line that
    answers it; None when the session has either missing. Skipped prompts: ones the scrub left
    fully "[MASKED]", and Yi's own subagent notifications (the "[subagent …]" shape the runtime
    inserts as user rows); a real prompt that starts with '[' is kept. Line breaks flatten to
    spaces, which the drive's one-step-per-line grammar cannot carry, and the note says when."""
    messages = [row["message"] for row in rows if row.get("type") == "message"]
    def typed(text: str) -> bool:
        return bool(text.replace(MASK, "").strip()) and not text.startswith("[subagent ")
    prompts = [text_of(m) for m in messages if m["role"] == "user" and typed(text_of(m))]
    replies = [m for m in messages if m["role"] == "assistant" and text_of(m)]
    if not prompts or not replies:
        return None
    reply = {key: replies[-1][key] for key in ("role", "api", "provider", "model", "usage",
                                              "stopReason", "timestamp") if key in replies[-1]}
    reply["content"] = [{"type": "text", "text": text_of(replies[-1])}]
    prompt = prompts[-1]
    note = ""
    if re.search(r"\r\n?|\n", prompt):
        prompt = re.sub(r"\r\n?|\n", " ", prompt)
        note = ("the recorded prompt had line breaks, flattened to spaces: the drive grammar"
                " carries one step per line")
    return prompt, reply, note


def harvest(session: Path, name: str, views: tuple[str, ...], size: str,
            out: Path = SCENES) -> list[Path]:
    """Writes a session as scenes: per view a seed cut at that view's end and a drive that snaps
    it; the `end` view then types the last prompt and plays the recorded reply as a cassette.
    Seed content goes through record.py's scrub, the redaction a recorded session already carries,
    so a seed from an unscrubbed input still lands masked."""
    if not NAME.fullmatch(name):
        sys.exit(f"name {name!r} is not [a-z0-9_-]+")
    raw = [record.scrub_values(json.loads(line)) for line in session.read_text().splitlines()]
    if raw[0].get("id") != MASK:
        print(f"[… {session.name} was not record.py-scrubbed: seed content is redacted]")
    rows = resumable(raw)
    where = out.relative_to(ROOT) if out.is_relative_to(ROOT) else out
    written, cuts = [], {}
    for view in views:
        kept = cut(rows, view)
        if kept is None:
            print(f"[… view {view} has no {'toolResult' if view == 'tool' else 'reply'} row:"
                  " no scene written]")
            continue
        if kept in cuts:
            print(f"[… view {view} cuts where {cuts[kept]} does: no scene written]")
            continue
        stem = f"harvest-{name}-{view}"
        flags = f"--size {size} --seed {where}/{stem}.jsonl"
        steps = ["wait-frame 5000 faux-1", f"snap {view}"]
        flat = ""
        again = continuation(rows) if view == "end" else None
        if again:
            prompt, reply, flat = again
            (out / f"{stem}.reply.jsonl").write_text(json.dumps(reply) + "\n")
            flags += f" --faux {where}/{stem}.reply.jsonl"
            # Incident: a turn's elapsed seconds read the wall clock and flipped between two runs;
            # a `tick` freezes it, and the reveal then moves only on ticks, so the scene waits on
            # the spinner coming and going rather than on a duration.
            steps += [f"type {prompt}", "key enter", "wait-frame 10000 Composing", "tick 30000",
                      "wait-idle 30000", "wait-frame 10000 !Composing", "snap continued"]
        (out / f"{stem}.jsonl").write_text("".join(json.dumps(row) + "\n" for row in rows[:kept]))
        (out / f"{stem}.drive").write_text(
            f"#! solo {flags}\n# Harvested from {session.name} by `tui_see.py harvest`: view {view}.\n"
            + (f"# {flat}\n" if flat else "")
            + "\n".join(steps + ["quit"]) + "\n")
        written.append(out / f"{stem}.drive")
        if kept < len(rows):
            print(cap_notice(kept, len(rows), "rows", f"{session.name} holds the whole session"))
    print("scenes: " + ", ".join(str(where / p.name) for p in written))
    return written


def selfcheck() -> None:
    """Fixtures are real snaps from `yi tui --headless`; their `.txt` comes from the Rust side."""
    runs = [SCENES / "empty", SCENES / "typed"]
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
    words = ["--size", "80x24", "--stub", "a.txt", "90", "--seed", "s"]
    assert take(words, "--stub", 2) == ["a.txt", "90"] and words == ["--size", "80x24", "--seed", "s"]
    assert take(words, "--stub", 2) == [] and len(words) == 4, "an absent flag took nothing"
    for refused in ("ctrl-1", "hyper-x", "f13"):
        try:
            key_bytes(refused)
        except ValueError:
            continue
        raise AssertionError(f"{refused} was encoded instead of refused")
    with tempfile.TemporaryDirectory() as tmp:
        scene = Path(tmp) / "composer-scene"
        shutil.copytree(runs[0], scene / "base")
        shutil.copytree(runs[1], scene / "head")
        diff(scene / "base", scene / "head")
        page = gallery(Path(tmp), Path(tmp) / "g.html").read_text()
        assert page.count("<figure>") == 3, "composer by two labels, answered by head alone"
        assert "1 changed region(s)" in page, "the head run's diff rides beside its snaps"
        assert "make the rail calmer" in page and "<title>Yi TUI gallery</title>" in page
    # A kitty placement split across two reads still disappears whole; text on either side stays.
    apc = ApcFilter()
    fed = apc.feed(b"left \x1b_Ga=p,i=8003;AAAA") + apc.feed(b"BBBB\x1b") + apc.feed(b"\\ right\x1b")
    assert fed + apc.feed(b"[0m") == b"left  right\x1b[0m", fed
    # Recorded on the term host: the orb's rows scrolled more of Ghostty's transcript away and its
    # status row names another cwd, yet the words agree; a screen from another moment is named.
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copytree(SCENES / "reveal-ghostty", tmp, dirs_exist_ok=True)
        assert drift(Path(tmp)) == [], drift(Path(tmp))
        shutil.copy(Path(tmp) / "term/snap-held.screen", Path(tmp) / "term/snap-done.screen")
        notes = drift(Path(tmp))
        assert len(notes) == 1 and notes[0].startswith("[… snap done: "), notes
    # The proxy opens a frozen scene with a frozen clock and spells a strip as the drive parses it.
    played = steps(SCENES / "reveal.drive")
    assert played.count("tick 0") == 1 and played[0] == "tick 0", played
    strip = played.index("snap reveal-00")
    assert played[strip:strip + 5] == ["snap reveal-00", "tick 160", "snap reveal-01", "tick 160",
                                       "snap reveal-02"], played
    assert "tick 0" not in steps(SCENES / "typed.drive")
    # The Ghostty replay starts from the seed as staged, whatever the headless drive appended.
    with tempfile.TemporaryDirectory() as tmp:
        seed = Path(tmp) / "s.jsonl"
        seed.write_text(json.dumps({"kind": "header", "version": 4, "id": FIXED_ID,
                                    "createdAt": 1790000000000, "cwd": tmp}) + "\n")
        session = stage_seed(Path(tmp) / "run", seed)
        grown = next((Path(tmp) / "run" / "sessions").glob(f"*/*_{session}.jsonl"))
        grown.write_text(grown.read_text() + '{"kind":"message"}\n')
        assert pristine_seed(Path(tmp) / "run", session).read_text() == seed.read_text()
    # Each terminal opens one window of the scene's size around the proxy; one Yi does not claim
    # is refused rather than played in whichever terminal happened to be the default.
    for name, size in (("ghostty", "--window-width=60"), ("kitty", "initial_window_width=60c"),
                       ("wezterm", "initial_cols=60")):
        argv = terminal_argv(name, "60", "12", ["python3", "proxy"], font="Noto Sans Mono")
        assert argv[0] == name and argv[-2:] == ["python3", "proxy"] and size in argv, argv
        assert sum("Noto Sans Mono" in arg for arg in argv) == 1, f"{name} ignored the font: {argv}"
    try:
        terminal_argv("iterm2", "60", "12", ["python3"])
        raise AssertionError("iterm2 was launched instead of refused")
    except ValueError:
        pass
    assert iterm_notice(False).startswith(f"[… kept {len(TERMINALS)} of {len(TERMINALS) + 1} terminals")
    # The ledger reads Yi's own escape spellings: a placement split across two reads counts once,
    # a transmit or delete is no placement, and an rgb red of 38 is not a second colour.
    with tempfile.TemporaryDirectory() as tmp:
        cast = Path(tmp) / "run.cast"
        events = ["\x1b[48;5;2;38;2;38;5;200mx\x1b[0m\x1b]2;yi\x07\x1b]133;A\x07",
                  "\x1b_Gf=32,o=z,s=4,v=4,a=t,i=7602,q=2,m=0;AAAA\x1b\\\x1b_Ga=p,i=80",
                  "00,p=1,q=2,C=1,c=2,r=1\x1b\\\x1b_Ga=p,i=7601,p=1,q=2\x1b\\\x1b[1;38;5;38m",
                  "\x1b_Ga=d,d=I,i=7601,q=2\x1b\\\x1b_Ga=p,i=7601,p=1,q=2,C=1,c=4,r=2\x1b\\\x1b]133;A\x07"]
        cast.write_text("\n".join([json.dumps({"version": 2}), *(json.dumps([0.1, "o", e]) for e in events)]))
        (Path(tmp) / "env.json").write_text(json.dumps({"TERM": "xterm-kitty", "COLORTERM": None}))
        seen = ledger(cast)
        assert seen["placed"] == [7601, 8000] and families(seen["placed"]) == "76xx ×1, 80xx ×1", seen
        assert seen["forms"] == {"rgb": 1, "256": 1} and seen["tier"] == "truecolor", seen
        assert seen["osc"] == {"2": 1, "133": 2} and seen["env"]["TERM"] == "xterm-kitty", seen
        line = ledger_line("kitty", seen)
        assert "TERM=xterm-kitty · colour tier truecolor (rgb 1, 256 1) · kitty placed 76xx" in line, line
        cast.write_text(json.dumps({"version": 2}) + "\n" + json.dumps([0.1, "o", "\x1b[38;5;38mx"]))
        assert ledger(cast)["tier"] == "256", ledger(cast)
    flat = ledger(runs[1] / "frames" / "snap-answered.cast")
    assert flat["tier"] == "16" and flat["placed"] == [] and flat["env"] == {}, flat
    # A scrubbed session harvests into seeds the store accepts: unique ids, each entry's parent
    # the entry before it (its records carry no parent), and each view cut where it says.
    fixture = ROOT / "crates/tui/tests/fixtures/sessions/01a0648f.jsonl"
    with tempfile.TemporaryDirectory() as tmp:
        scenes = harvest(fixture, "x", VIEWS, "80x24", Path(tmp))
        assert [p.name for p in scenes] == [f"harvest-x-{v}.drive" for v in VIEWS], scenes
        seeds = {v: [json.loads(line) for line in (Path(tmp) / f"harvest-x-{v}.jsonl").read_text().splitlines()]
                 for v in VIEWS}
        for view, rows in seeds.items():
            assert re.fullmatch(r"[0-9a-f-]+", rows[0]["id"]), f"{view}: header id {rows[0]['id']}"
            ids = [row["id"] for row in rows[1:]]
            assert len(set(ids)) == len(ids) and MASK not in ids, f"{view}: duplicate or masked ids"
            leaf = None
            for row in rows[1:]:
                if row["kind"] == "entry":
                    assert row["parentId"] == leaf, f"{view} seq {row['seq']}: parent {row['parentId']}"
                    leaf = row["id"]
        original = [json.loads(line) for line in fixture.read_text().splitlines()]
        assert original[0]["id"] == MASK and len(seeds["end"]) == len(original), "end keeps every row"
        results = [r for r in original if r.get("message", {}).get("role") == "toolResult"]
        shown = [r for r in seeds["tool"] if r.get("message", {}).get("role") == "toolResult"][-1]
        assert shown["message"] == max(results, key=lambda r: len(json.dumps(r)))["message"], "tool view"
        replies = [r["message"] for r in original if r.get("message", {}).get("role") == "assistant"]
        last = [r["message"] for r in seeds["long"] if r.get("message", {}).get("role") == "assistant"][-1]
        assert len(text_of(last)) == max(len(text_of(m)) for m in replies), "long view ends on the longest reply"
        surface, flags = header(scenes[0])
        cassette = Path(flags[flags.index("--faux") + 1])
        reply = json.loads(cassette.read_text())
        assert surface == "solo" and Path(flags[flags.index("--seed") + 1]).exists(), flags
        assert reply["content"][0]["text"].startswith("Tooling works. Results:"), reply
        assert all("--faux" not in header(p)[1] for p in scenes[1:]), "only the end view continues"
    import contextlib, io

    def recorded(tmp: Path, name: str, messages: list[dict]) -> Path:
        path = Path(tmp) / name
        rows = [{"kind": "header", "version": 4, "id": MASK, "createdAt": 1790000000000, "cwd": "."}]
        rows += [{"kind": "entry", "type": "message", "id": MASK, "parentId": MASK, "seq": n,
                  "message": m} for n, m in enumerate(messages, 1)]
        path.write_text("".join(json.dumps(r) + "\n" for r in rows))
        return path
    with tempfile.TemporaryDirectory() as tmp:
        chat = recorded(tmp, "chat.jsonl", [{"role": "user", "content": "hi"},
                                            {"role": "assistant", "content": "hello"}])
        with contextlib.redirect_stdout(io.StringIO()) as seen:
            assert harvest(chat, "y", ("tool",), "80x24", Path(tmp)) == [], "chat-only: no tool scene"
        assert "no scene written" in seen.getvalue(), seen.getvalue()
        spoken = recorded(tmp, "spoken.jsonl", [{"role": "user", "content": "hi"}])
        with contextlib.redirect_stdout(io.StringIO()) as seen:
            assert harvest(spoken, "y", ("long",), "80x24", Path(tmp)) == [], "no reply: no long scene"
        assert "no scene written" in seen.getvalue(), seen.getvalue()
        bracket = recorded(tmp, "bracket.jsonl", [
            {"role": "user", "content": "[review notes]\nkeep  the  spacing"},
            {"role": "assistant", "content": "on it"},
        ])
        harvest(bracket, "b", ("end",), "80x24", Path(tmp))
        drive = (Path(tmp) / "harvest-b-end.drive").read_text()
        assert "type [review notes] keep  the  spacing\n" in drive, drive
        assert "line breaks" in drive, drive
        secret = recorded(tmp, "secret.jsonl", [
            {"role": "user", "content": "run the tool"},
            {"role": "toolResult", "toolCallId": "call_x", "toolName": "bash",
             "content": "api_key = hunter2supersecret\n"},
            {"role": "assistant", "content": "done"},
        ])
        harvest(secret, "z", ("end",), "80x24", Path(tmp))
        seed = (Path(tmp) / "harvest-z-end.jsonl").read_text()
        assert "hunter2supersecret" not in seed and MASK in seed, seed
    keep_harvest = globals()["harvest"]
    try:
        globals()["harvest"] = lambda *args, **kw: (_ for _ in ()).throw(RuntimeError("harvest ran"))
        try:
            main(["harvest", str(fixture)])
        except SystemExit as exit_code:
            assert "--view is required" in str(exit_code), exit_code
        else:
            raise AssertionError("harvest without --view ran all views")
    finally:
        globals()["harvest"] = keep_harvest
    print("tui_see selfcheck: ok")


def main(argv: list[str]) -> None:
    if argv[:1] == ["--selfcheck"]:
        return selfcheck()
    if argv[:1] == ["monkey"]:
        import tui_monkey
        return tui_monkey.monkey(argv[1:])
    if argv[:1] == ["diff"] and len(argv) == 3:
        diff(Path(argv[1]).resolve(), Path(argv[2]).resolve())
        return None
    if argv[:1] == ["term-inner"]:
        return term_inner(argv[1], argv[2], Path(argv[3]), argv[4], argv[5:])
    if argv[:1] == ["harvest"] and len(argv) >= 2:
        opts = dict(zip(argv[2::2], argv[3::2]))
        unknown = set(opts) - {"--name", "--view", "--size"}
        if unknown or len(argv[2:]) % 2 or "--view" not in opts or opts["--view"] not in VIEWS:
            sys.exit(f"harvest: unknown or valueless flags {sorted(unknown) or argv[-1:]}, --view"
                     f" is required and one of {', '.join(VIEWS)}")
        session = Path(argv[1]).resolve()
        harvest(session, opts.get("--name", session.stem.lower()), (opts["--view"],),
                opts.get("--size", "100x40"))
        return None
    if argv[:1] == ["proxy"]:
        out = Path(argv[2])
        try:
            return proxy(Path(argv[1]), out, argv[argv.index("--") + 1:])
        except Exception:
            import traceback
            # Ghostty owns this process's stderr, and its surface closes with the error on it.
            (out / "done").write_text(traceback.format_exc())
            raise
    if argv[:1] == ["gallery"]:
        root = Path(argv[1] if len(argv) > 1 else "target/see")
        gallery(root, Path(argv[2]) if len(argv) > 2 else root / "gallery.html")
        return None
    if argv[:1] == ["terminals"] and len(argv) in (2, 4, 6):
        opts = dict(zip(argv[2::2], argv[3::2]))
        if set(opts) - {"--out", "--bin"}:
            sys.exit(f"terminals: unknown flags {sorted(set(opts) - {'--out', '--bin'})}")
        terminals(Path(argv[1]), Path(opts.get("--out", "target/see-terminals")),
                  Path(opts.get("--bin", "target/debug/yi")).resolve())
        return None
    if argv[:1] == ["watch"] and len(argv) in (2, 4):
        watch(Path(argv[1]), argv[3] if argv[2:3] == ["--host"] else "mem")
        return None
    video = "--video" in argv
    argv = [arg for arg in argv if arg != "--video"]
    if argv[:1] == ["run"] and len(argv) >= 2:
        opts = dict(zip(argv[2::2], argv[3::2]))
        unknown = set(opts) - {"--label", "--out", "--bin", "--host", "--terminal"}
        if unknown or len(argv[2:]) % 2:
            sys.exit(f"run: unknown or valueless flags {sorted(unknown) or argv[-1:]}")
        run(Path(argv[1]), opts.get("--label", "head"), Path(opts.get("--out", "target/see")),
            Path(opts.get("--bin", "target/debug/yi")).resolve(), opts.get("--host", "mem"), video,
            opts.get("--terminal", "ghostty"))
        return None
    print(__doc__)
    sys.exit(2)


if __name__ == "__main__":
    main(sys.argv[1:])
