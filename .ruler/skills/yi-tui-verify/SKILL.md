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
6. A UI change that a person has to look at gets a proof artifact from
   `just tui-see run <scene>` (step 7): each `snap` is a still, and `--video` keeps the
   whole run as motion (`run.gif` from agg headless, `run.webp` from Ghostty on
   `--host term`). Attach them to the PR, or let the forge's `tui` job post them —
   frame dumps prove the contract held, they do not show whether it looks right. Render
   with a terminal emulator, which draws the cell grid the TUI actually painted.
   `freeze` was tried and dropped — it re-renders text in its own font and chrome, so
   the still was not 1:1 (the composer border ran off the edge) and `-c full` wrote a
   0-byte file while exiting 0. Pace the typing with `type-ms <n>` (0 by default, so
   assertion scripts stay instant), and put `--deadline <secs>` in the scene's `#!` line
   when the beats outlast the 60 s default. A wide layout is `#! console --size 254x40`.
   `scripts/proof/demo.drive` is the worked example.
7. To see what a change did, diff snaps rather than eyeballing whole frames:
   `just tui-see run <scene> --label base`, edit, `just tui-see run <scene>`, then
   `just tui-see diff target/see/<scene>/base target/see/<scene>/head`. The summary lists every
   changed row with its text and style changes (`cols 25-25: plain → reverse`), the changed
   regions, and crops of each region before and after (agg pads its stills, so a crop is within
   half a row of exact); Read the crops, not the whole frame.
   Scenes live beside the proofs: `#! solo|console <flags>` on top, `snap <name>` where a frame
   matters, `--seed <session.jsonl>` in the `#!` line to start from a real session (without one,
   a solo scene starts from an empty session under a fixed id, so its badge colour is stable).
   `--host term` replays the scene in real Ghostty inside the yi-term container (Xvfb, software
   GL) and swaps in its grabs, so kitty images, the orb and true colour are in the crops. Build it
   once with `just yi-term`; it matches the host's architecture, because Ghostty under emulation
   took 30 s to map a window.
8. Motion and in-flight states: put `--faux <cassette.jsonl>` in the `#!` line, `hold
   text_delta 2` before the prompt, snap while it is held, `release`, then `snap-strip reveal 6
   80`. Any `tick` freezes the clock for the whole scene: `wait-idle` does not move it, so `tick`
   past the reveal before waiting, and a `tick` waits (5 s at most) for an armed hold to be
   reached. Hold is solo only; the console's model lives in the daemon. A `--faux` cassette cut
   from a recorded session file streams each reply over the time it took, and a `tick` moves that
   clock (`paced.drive`). On the term host the proxy sends these steps and `wait-idle` over the
   real binary's `--control <socket>`, which plays them on the drive's engine and answers `ok` or
   `error: …`, so a frozen scene's Ghostty grabs land on its headless frames; a snap whose words
   part from the headless one is named.
9. Iterating: `just tui-see watch <scene> [--host term]` keeps a `base` run and, on every save
   under crates/tui, crates/console or crates/orb, rebuilds, reruns as `head`, diffs and rewrites
   `target/see/gallery.html`. `just tui-see gallery [root] [page]` builds that page on its own:
   each scene, each snap by label side by side, the head's cell diff beside it, pictures inlined,
   so it can be published as one page for the owner to comment on. `--video` with `--host term`
   keeps the whole run as `run.webp` (cropped to the window, under the 512 KB blob cap for a
   README); it needs at least one `snap` to find the window. On that host `wait-idle` is the
   binary's own answer, so a turn still finishing its checkpoint is waited out, not read as idle;
   the binary runs in a small cwd there, because a checkpoint of the mounted repo took minutes.
   Harvest a scene from a recorded session instead of writing one:
   `just tui-see harvest crates/tui/tests/fixtures/sessions/<id>.jsonl --name N [--view
   end|tool|long]` writes `harvest-N-<view>` into scripts/proof/tui_see. A solo screen has no
   scroll, so each view is its own seed cut where it matters: `end` the whole session, `tool`
   through the largest tool result, `long` through the longest reply (the cut is named on the
   way out). The scrub masks ids the store refuses, so the seed gets unique ids and a chain. The
   `end` scene also retypes the last prompt and plays the last recorded reply as its cassette;
   that turn freezes the clock, because its elapsed seconds read the wall clock.
   `tui_see.py` sets `YI_FIXED_IDS=1` for the serve it starts, so console sessions count ids up
   from 1 and two runs draw the same avatar colours.
10. Fuzzing: `just tui-see monkey --runs 20 --seed 7 [--surface solo|console]` plays seeded random key
   runs through the headless loops and reports only a crash, a hang or a control character in a
   cell; the seed's script is `target/see/monkey/<seed>/<surface>.drive`, replayed by `tui_see.py run`.
