#!/usr/bin/env python3
"""Drive the real yi TUI under a PTY that behaves like a terminal emulator.

`script(1)` cannot verify the TUI: nothing answers the ESC[6n cursor-position
query the inline viewport blocks on, and the PTY winsize is 0x0 so every
frame renders zero-width. This harness answers CPR, sets a real winsize,
optionally injects key bytes, and prints the captured output with escape
sequences stripped.

Usage:
  python3 scripts/tui_pty.py [--rows 24] [--cols 80] [--seconds 8]
      [--send-quit] -- <yi args...>
Example:
  python3 scripts/tui_pty.py --send-quit -- \
      tui --model faux/faux-1 --session-dir /tmp/yi-pty "ping"
"""

import argparse
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
    parser.add_argument("--send-quit", action="store_true",
                        help="send double ctrl-c after seconds/2")
    parser.add_argument("--resize", metavar="COLSxROWS", action="append",
                        default=[],
                        help="resize the pty (and SIGWINCH) mid-run; repeatable")
    parser.add_argument("--expect", action="append", default=[],
                        help="fail unless the captured screen contains this "
                             "text; repeatable")
    parser.add_argument("--binary", default="./target/debug/yi")
    parser.add_argument("--term", default="xterm-256color",
                        help="TERM for the child; use xterm-kitty to exercise "
                             "the kitty graphics path")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    yi_args = [a for a in options.args if a != "--"]

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = options.term
        os.execvp(options.binary, [options.binary, *yi_args])

    fcntl.ioctl(fd, termios.TIOCSWINSZ,
                struct.pack("HHHH", options.rows, options.cols, 0, 0))

    out = bytearray()
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
        ready, _, _ = select.select([fd], [], [], 0.1)
        if ready:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            out += chunk
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
    # A killed child exits 0 here, so without --expect a run that rendered
    # nothing still passes; the journey lane needs the screen asserted.
    missing = [needle for needle in options.expect if needle not in plain]
    if missing:
        print(f"missing from the captured screen: {missing}")
        return 1
    return 0 if exit_status in (0, None) else 1


if __name__ == "__main__":
    sys.exit(main())
