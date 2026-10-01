---
name: yi-tui-verify
description: Verify a Yi TUI change against the rendered UI — headless drive mode, frame assertions, and the PTY harness for the true-terminal path
---

# Verifying TUI changes

1. Build, then drive the real binary headless:
   `./target/debug/yi tui --headless --model faux/faux-1 --session-dir <tmp> \
      --keys <script> --frames <dir> "<prompt>"`
   Script grammar, one step per line: `key <spec>` (keymap grammar: `enter`,
   `ctrl-t`, `alt-down`), `type <text>`, `wait <ms>`, `wait-idle <ms>`
   (times out red), `quit`. Frames land as plain-text screens, one file per
   changed frame — grep them for the contract, don't eyeball only.
2. The drive loop is the real loop: same App, reduce, and draw path; only
   the terminal is in-memory and the keyboard is the script. What it cannot
   exercise: raw mode, CPR, kitty flags, kitty graphics.
3. For those, run `scripts/tui_pty.py` — it forks the binary under a real
   PTY, answers ESC[6n cursor queries like an emulator, sets TIOCSWINSZ, and
   injects bytes. `script(1)` alone cannot verify the TUI: nothing answers
   the CPR and the winsize is 0×0.
4. In-process tests use `VT100Backend` (tests/common) for escape-level
   assertions and `App::new` + `reduce_agent`/`take_commits` for reduction
   contracts. Any viewport-geometry test pre-scrolls the parser to a nonzero
   cursor row first. Assert a row with `VT100Backend::row_text(row)`, never a
   substring over the whole screen: the screen proved "python" was somewhere
   while the row it sat on said something else. `common::replay_stream` drives
   a recorded session from crates/tui/tests/fixtures/sessions/ through the same
   reducer.
5. Animation and orb changes: the engine is pinned by the golden-vector
   parity test; a rendering change is verified by frame dumps or the PTY
   harness, never by re-deriving geometry.
6. A UI change that a person has to look at gets a proof artifact:
   `just tui-proof <script>` runs the same drive loop with `--record`
   (asciicast v2 of the whole run) and `--snap` (the last frame as a cast
   of its own), then renders `run.gif` and `still.gif` with agg. Attach one
   to the PR — frame dumps prove the contract held, they do not show
   whether it looks right. Both artifacts go through agg because agg is a
   terminal emulator: it renders the cell grid the TUI actually painted.
   `freeze` was tried and dropped — it re-renders text in its own font and
   chrome, so the still was not 1:1 (the composer border ran off the edge)
   and `-c full` wrote a 0-byte file while exiting 0. Pace the typing with
   `type-ms <n>` (0 by default, so assertion scripts stay instant) and
   raise `--deadline <secs>` when the beats outlast the 60 s default.
   The headless screen is 80x24; `--size COLSxROWS` renders another, and
   `just console-proof <script> <out> 254x40` passes it through, for a
   layout that only shows on a wide screen.
   `scripts/proof/demo.drive` is the worked example. A test replays both
   casts and asserts they land on the frame the assertions ran against.
