#!/usr/bin/env python3
"""Drive the real yi TUI under a PTY that behaves like a terminal emulator.

`script(1)` cannot verify the TUI: nothing answers the ESC[6n cursor-position
query the inline viewport blocks on, and the PTY winsize is 0x0 so every
frame renders zero-width. This harness answers CPR, sets a real winsize,
optionally injects key bytes, and prints the captured output with escape
sequences stripped.

Usage:
  python3 scripts/tui_pty.py [--rows 24] [--cols 80] [--seconds 8]
      [--send-quit] [--frame-audit] -- <yi args...>
  python3 scripts/tui_pty.py --selfcheck
Example:
  python3 scripts/tui_pty.py --send-quit -- \
      tui --model faux/faux-1 --session-dir /tmp/yi-pty "ping"
"""

import argparse
import atexit
import shutil
import tempfile
import unicodedata
import fcntl
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time


ORB_COLS, ORB_ROWS = 10, 5
CHECKS = ("text under placement", "placement off the orb slot",
          "two placements of one family on different rows",
          "content moved more than one row or down", "scrollback mixes prose with chrome")
TOKEN = re.compile(
    rb"\x1b\[([?>=<]?)([0-9;:]*)[ -/]*([@-~])"
    rb"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"
    rb"|\x1b_([^\x1b]*)\x1b\\"
    rb"|\x1bP[^\x1b]*\x1b\\"
    rb"|\x1b([78M])"
    rb"|([\r\n\b])"
    rb"|([^\x1b\r\n\b]+)", re.S)
CHROME = re.compile(r"[╭╰╮╯]|─{3,}|\[esc\] interrupt|esc again to interrupt")
SLOT_ABOVE = 2
SOLO_ORB = ("7601", "7602")


def orb_family(image: str):
    """The double-buffered pair an orb image id belongs to, or None for a logo,
    avatar or notebook image: solo's 7601/7602, console pane k's 7800+2k/7801+2k."""
    if image in SOLO_ORB:
        return "solo"
    number = int(image) if (image or "").isdigit() else 0
    return f"pane {(number - 7800) // 2}" if 7800 <= number < 8000 else None


class Screen:
    """Minimal VT: cursor, DECSTBM, SU/SD, ED/EL, kitty placements. Ghostty 1.3.1
    ignores DECSTBM when top >= bottom; a scroll whose region top is row 0 pushes
    the rows into scrollback; a placement follows a scroll only when fully inside."""

    def __init__(self, rows: int, cols: int):
        self.rows, self.cols = rows, cols
        self.grid = [[" "] * cols for _ in range(rows)]
        self.y = self.x = 0
        self.top, self.bot = 0, rows - 1
        self.saved = (0, 0)
        self.sync = False
        self.wrap = False
        self.place = {}
        self.scrollback = []

    def text(self, y: int) -> str:
        return "".join(self.grid[y])

    def scroll(self, n: int, down: bool = False):
        t, b = self.top, self.bot
        for _ in range(n):
            if down:
                del self.grid[b]
                self.grid.insert(t, [" "] * self.cols)
            else:
                if t == 0:
                    self.scrollback.append("".join(self.grid[0]).rstrip())
                del self.grid[t]
                self.grid.insert(b, [" "] * self.cols)
        for key, cell in list(self.place.items()):
            row, _, high, _ = cell
            if t <= row and row + high - 1 <= b:
                cell[0] += -n if not down else n
                if cell[0] + high - 1 < t:
                    del self.place[key]

    def linefeed(self):
        if self.y == self.bot:
            self.scroll(1)
        elif self.y < self.rows - 1:
            self.y += 1

    def put(self, ch: str):
        wide = 2 if unicodedata.east_asian_width(ch) in "WF" else 1
        if unicodedata.combining(ch):
            return
        if self.wrap:
            self.x, self.wrap = 0, False
            self.linefeed()
        self.grid[self.y][self.x] = ch
        if wide == 2 and self.x + 1 < self.cols:
            self.grid[self.y][self.x + 1] = ""
        self.x += wide
        if self.x >= self.cols:
            self.x, self.wrap = self.cols - 1, True

    def csi(self, private: str, params: str, final: str):
        p = [int(v) if v else 0 for v in params.split(";")] if params else []
        arg = lambda i, d=1: p[i] if i < len(p) and p[i] else d
        self.wrap = False
        last_row, last_col = self.rows - 1, self.cols - 1
        if private:
            if final in "hl" and 2026 in p:
                self.sync = final == "h"
                return "sync_" + final
        elif final in "Hf":
            self.y, self.x = min(arg(0) - 1, last_row), min(arg(1) - 1, last_col)
        elif final == "A":
            self.y = max(self.y - arg(0), 0)
        elif final == "B":
            self.y = min(self.y + arg(0), last_row)
        elif final == "C":
            self.x = min(self.x + arg(0), last_col)
        elif final == "D":
            self.x = max(self.x - arg(0), 0)
        elif final == "G":
            self.x = min(arg(0) - 1, last_col)
        elif final == "d":
            self.y = min(arg(0) - 1, last_row)
        elif final == "J" and (arg(0, 0) in (0, 2)):
            if arg(0, 0) == 2:
                self.grid = [[" "] * self.cols for _ in range(self.rows)]
                self.place.clear()
            else:
                self.grid[self.y][self.x:] = [" "] * (self.cols - self.x)
                for r in range(self.y + 1, self.rows):
                    self.grid[r] = [" "] * self.cols
        elif final == "K":
            start, stop = {0: (self.x, self.cols), 1: (0, self.x + 1)}.get(arg(0, 0), (0, self.cols))
            self.grid[self.y][start:stop] = [" "] * (stop - start)
        elif final == "r":
            top, bot = arg(0) - 1, arg(1, self.rows) - 1
            if top < bot < self.rows:
                self.top, self.bot = top, bot
            self.y = self.x = 0
        elif final in "ST":
            self.scroll(arg(0), down=final == "T")
        return None

    def kitty(self, body: str):
        keys = dict(kv.split("=", 1) for kv in body.split(";", 1)[0][1:].split(",") if "=" in kv)
        if keys.get("a") in ("p", "T"):
            self.place[(keys.get("i"), keys.get("p", "1"))] = [
                self.y, self.x, int(keys.get("r", ORB_ROWS)), int(keys.get("c", ORB_COLS))]
        elif keys.get("a") == "d" and keys.get("d") in ("i", "I"):
            for key in [k for k in self.place if k[0] == keys.get("i")]:
                del self.place[key]


def content_shift(prev: list, cur: list) -> int:
    """Rows the text moved between two presents: positive up, negative down; 0
    when fewer than three identical text rows tie the frames together."""
    best, shift = 2, 0
    for k in range(-len(cur) + 1, len(cur)):
        pairs = zip(prev[max(k, 0):], cur[max(-k, 0):])
        same = sum(1 for a, b in pairs if a == b and a.strip())
        if same > best:
            best, shift = same, k
    return shift


def frame_audit(out: bytes, bounds: list, rows: int, cols: int, off=()) -> tuple:
    """Replay the capture and judge every presented frame: one at each ?2026l and
    at each read boundary (`bounds`: output length after each read) while sync is off.
    Returns the hits per check and the placements still live at the end."""
    screen = Screen(rows, cols)
    found = {name: [] for name in CHECKS}
    state = {"frame": 0, "prev": None, "dirty": False}

    def flag(check: str, at: int, note: str):
        if check not in off:
            found[check].append(f"frame {state['frame']} (byte {at}): {note}")

    def present(at: int):
        state["frame"] += 1
        state["dirty"] = False
        text = [screen.text(y) for y in range(rows)]
        label = next((y for y, line in enumerate(text)
                      if "[esc] interrupt" in line or "esc again to interrupt" in line), None)
        for (image, _), (row, col, high, wide) in screen.place.items():
            ink = sum(1 for y in range(max(row, 0), min(row + high, rows))
                      for ch in screen.grid[y][col:col + wide] if ch.strip())
            if ink:
                flag(CHECKS[0], at, f"image {image} at row {row} covers {ink} text cells")
            if image in SOLO_ORB and label is not None and row != label - SLOT_ABOVE:
                flag(CHECKS[1], at, f"image {image} at row {row}, slot is row {label - SLOT_ABOVE}")
        family = {}
        for (image, _), cell in screen.place.items():
            if orb_family(image):
                family.setdefault(orb_family(image), set()).add(cell[0])
        for fam, spots in family.items():
            if len(spots) > 1:
                flag(CHECKS[2], at, f"orb {fam} placed on rows {sorted(spots)}")
        prev = state["prev"]
        if prev is not None and prev != text:
            shift = content_shift(prev, text)
            if shift > 1 or shift < 0:
                flag(CHECKS[3], at, f"text moved {shift} rows")
        state["prev"] = text

    edges = iter(bounds)
    edge = next(edges, None)
    for token in TOKEN.finditer(out):
        while edge is not None and token.start() >= edge:
            if not screen.sync and state["dirty"]:
                present(edge)
            edge = next(edges, None)
        if token.group(3):
            verdict = screen.csi(token.group(1).decode(), token.group(2).decode(), token.group(3).decode())
            if verdict == "sync_l":
                present(token.start())
            elif not screen.sync:
                state["dirty"] = True
        elif token.group(4) is not None:
            screen.kitty(token.group(4).decode("latin-1"))
            state["dirty"] = state["dirty"] or not screen.sync
        elif token.group(5):
            mark = token.group(5)
            if mark == b"7":
                screen.saved = (screen.y, screen.x)
            elif mark == b"8":
                screen.y, screen.x = screen.saved
            elif screen.y == screen.top:
                screen.scroll(1, down=True)
            else:
                screen.y = max(screen.y - 1, 0)
        elif token.group(6):
            if token.group(6) == b"\r":
                screen.x, screen.wrap = 0, False
            elif token.group(6) == b"\n":
                screen.linefeed()
            else:
                screen.x = max(screen.x - 1, 0)
        elif token.group(7):
            for ch in token.group(7).decode("utf-8", "replace"):
                screen.put(ch)
            state["dirty"] = state["dirty"] or not screen.sync
    if state["dirty"] and not screen.sync:
        present(len(out))
    for line in screen.scrollback:
        stripped = re.sub(r"[╭╰╮╯│─\s]+", " ", line)
        if CHROME.search(line) and re.search(r"[A-Za-z]{3,}", CHROME.sub("", stripped)):
            if CHECKS[4] not in off:
                found[CHECKS[4]].append(f"scrollback row: {line.strip()[:70]!r}")
    return found, screen.place


def selfcheck(off=frozenset()) -> int:
    """Each check fires on a stream built to trip only it, stays quiet on the
    clean twin, and is refuted by running the same expectations with it disabled."""
    at = lambda r, c: f"\x1b[{r};{c}H"
    orb = lambda i: f"\x1b_Ga=p,i={i},p=1,c=10,r=5\x1b\\"
    sync = lambda body: "\x1b[?2026h" + body + "\x1b[?2026l"
    block = lambda top: "".join(at(top + n, 1) + word for n, word in enumerate(("aaaa", "bbbb", "cccc", "dddd")))
    label = at(8, 12) + "[esc] interrupt"
    slot = at(6, 1) + orb(7601)
    clean = sync(label + slot) + sync(at(1, 1) + "hello")
    cases = {
        CHECKS[0]: sync(label + at(7, 3) + "x" + slot),
        CHECKS[1]: sync(label + at(5, 1) + orb(7601)),
        CHECKS[2]: sync(label + slot + at(2, 1) + orb(7602)),
        CHECKS[3]: sync(block(1)) + sync("\x1b[2J" + block(4)),
        CHECKS[4]: sync(at(1, 1) + "prose words ╭────╮" + at(12, 1) + "\n"),
    }
    run = lambda stream, bounds=(): frame_audit(stream.encode(), list(bounds), 12, 40, off)[0]
    for check, stream in cases.items():
        assert run(stream)[check], f"{check}: its stream passed"
    unsynced = label + slot + at(7, 3) + "x"
    assert run(unsynced, [len((label + slot).encode()), len(unsynced.encode())])[CHECKS[0]], \
        "text under a placement went unseen without ?2026"
    quiet = (clean, sync(block(2)) + sync("\x1b[2J" + block(1)),
             sync(at(1, 1) + "prose words" + at(12, 1) + "\n" + at(1, 1) + "╭────╮"),
             sync(label + slot + at(1, 1) + orb(7610) + at(3, 1) + orb(7611)
                  + at(1, 20) + orb(7802) + at(3, 20) + orb(7804)))
    for stream in quiet:
        assert not any(run(stream).values()), f"clean stream flagged: {run(stream)}"
    if not off:
        for check in CHECKS:
            try:
                selfcheck(frozenset({check}))
            except AssertionError:
                continue
            raise SystemExit(f"selfcheck: disabling {check!r} changed nothing")
        print("tui_pty selfcheck: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, default=24)
    parser.add_argument("--cols", type=int, default=80)
    parser.add_argument("--seconds", type=float, default=8.0)
    parser.add_argument("--send", action="append", default=[],
                        help="key bytes to send mid-run, repeatable and evenly "
                             "spaced; python escapes are decoded (\\x1b, \\r)")
    parser.add_argument("--raw", help="write the child's untouched output here, "
                                     "so escape sequences can be counted")
    parser.add_argument("--stdin", action="store_true",
                        help="pass this process's stdin to the child once it has drawn, so "
                             "a caller types in step with what it observes")
    parser.add_argument("--send-quit", action="store_true",
                        help="send double ctrl-c after seconds/2")
    parser.add_argument("--resize", metavar="COLSxROWS", action="append",
                        default=[],
                        help="resize the pty (and SIGWINCH) mid-run; repeatable")
    parser.add_argument("--expect", action="append", default=[],
                        help="fail unless the captured screen contains this "
                             "text; repeatable")
    parser.add_argument("--kitty-ledger", action="store_true",
                        help="print the kitty image placements the frame replay "
                             "leaves live at exit")
    parser.add_argument("--frame-audit", action="store_true",
                        help="replay the output and judge every presented frame: "
                             "text under a kitty placement, placements off the orb "
                             "slot, split families, multi-row jumps, chrome in "
                             "scrollback; exits 1 on any hit")
    parser.add_argument("--selfcheck", action="store_true",
                        help="run each audit check against a synthetic stream, "
                             "then again with the check disabled")
    parser.add_argument("--binary", default="./target/debug/yi")
    parser.add_argument("--term", default="xterm-256color",
                        help="TERM for the child; use xterm-kitty to exercise "
                             "the kitty graphics path")
    parser.add_argument("--home", help="HOME for the child; default a fresh "
                                       "temporary directory, never the caller's")
    parser.add_argument("--lanes", action="store_true",
                        help="let the child claim a lane; without it --here is "
                             "passed so a killed run leaves no orphan")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    if options.selfcheck:
        return selfcheck()
    yi_args = [a for a in options.args if a != "--"]
    home = options.home
    if not home:
        home = tempfile.mkdtemp(prefix="yi-pty-home-")
        atexit.register(shutil.rmtree, home, True)
    if not os.path.isabs(home):
        parser.error(f"--home must be absolute, not {home!r}: a relative HOME plants "
                     "a worktree beside the repository")
    if not options.lanes and "--here" not in yi_args:
        yi_args = [*yi_args, "--here"]

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = options.term
        os.environ["HOME"] = home
        os.execvp(options.binary, [options.binary, *yi_args])

    fcntl.ioctl(fd, termios.TIOCSWINSZ,
                struct.pack("HHHH", options.rows, options.cols, 0, 0))

    out = bytearray()
    bounds = []
    start = time.time()
    sent_quit = False
    exit_status = None
    pending_resizes = []
    for spec in options.resize:
        cols, _, rows = spec.partition("x")
        pending_resizes.append((int(cols), int(rows)))
    resize_step = options.seconds / (len(pending_resizes) + 2) if pending_resizes else 0
    next_resize_at = resize_step
    pending_sends = [bytes(spec, "utf-8").decode("unicode_escape").encode("latin-1")
                     for spec in options.send]
    send_step = options.seconds / (len(pending_sends) + 2) if pending_sends else 0
    next_send_at = send_step

    while time.time() - start < options.seconds:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            exit_status = status
            break
        # Keys sent before the child draws reach a tty still in cooked mode, which joins them.
        typing = [0] if options.stdin and out else []
        ready, _, _ = select.select([fd, *typing], [], [], 0.1)
        if 0 in ready:
            keys = os.read(0, 65536)
            if keys:
                os.write(fd, keys)
            else:
                options.stdin = False
        if fd in ready:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            out += chunk
            bounds.append(len(out))
            for _ in range(chunk.count(b"\x1b[6n")):
                os.write(fd, b"\x1b[%d;1R" % options.rows)
        if pending_resizes and time.time() - start > next_resize_at:
            cols, rows = pending_resizes.pop(0)
            fcntl.ioctl(fd, termios.TIOCSWINSZ,
                        struct.pack("HHHH", rows, cols, 0, 0))
            os.kill(pid, signal.SIGWINCH)
            next_resize_at += resize_step
        if pending_sends and time.time() - start > next_send_at:
            os.write(fd, pending_sends.pop(0))
            next_send_at += send_step
        if (options.send_quit and not sent_quit
                and time.time() - start > options.seconds / 2):
            os.write(fd, b"\x03")
            time.sleep(0.25)
            os.write(fd, b"\x03")
            sent_quit = True

    try:
        while True:
            ready, _, _ = select.select([fd], [], [], 0.3)
            if not ready:
                break
            chunk = os.read(fd, 65536)
            if not chunk:
                break
            out += chunk
            bounds.append(len(out))
    except OSError:
        pass
    if exit_status is None:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    if options.raw:
        with open(options.raw, "wb") as handle:
            handle.write(out)
    text = out.decode("utf-8", "replace")
    plain = re.sub(r"\x1b_G[^\x1b]*\x1b\\", "<kitty-image>", text)
    plain = re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b[<>=()78]|\x1b\\", "", plain)
    print(f"exit: {exit_status if exit_status is not None else 'killed'}")
    print(f"bytes: {len(out)}")
    print(plain)
    sizes = [(options.cols, options.rows), *(tuple(map(int, spec.split("x"))) for spec in options.resize)]
    hits, live = frame_audit(bytes(out), bounds, max(r for _, r in sizes), max(c for c, _ in sizes))
    if options.kitty_ledger:
        print(f"kitty placements live: {len(live)}")
        for (image, placement), (row, col, _, _) in sorted(live.items()):
            print(f"  image {image} placement {placement} at row {row + 1} col {col + 1}")
    audit_failed = False
    if options.frame_audit:
        for check, found in hits.items():
            print(f"frame audit: {len(found):4d}  {check}")
            for hit in found[:3]:
                print(f"    {hit}")
            audit_failed = audit_failed or bool(found)
    # A killed child exits 0 here, so without --expect a run that rendered
    # nothing still passes; the journey lane needs the screen asserted.
    missing = [needle for needle in options.expect if needle not in plain]
    if missing:
        print(f"missing from the captured screen: {missing}")
        return 1
    if audit_failed:
        return 1
    return 0 if exit_status in (0, None) else 1


if __name__ == "__main__":
    sys.exit(main())
