---
name: yi-tui-look
description: Iterate a Yi TUI change on how it looks, with the term recorder, and the rules the title animation taught
---

# Looking at a TUI change

`yi-tui-verify` proves a contract held. This is for the other half: making a surface look right.
The title animation took nine rounds because each one was judged by the owner; every rule below
is one a round bought. Mechanics (scene grammar, `diff`, `watch`, `gallery`) live in
`yi-tui-verify` steps 6 to 9; this is the order to use them in and what to look for.

## The loop

1. **A scene per moment.** One `snap` per second worth seeing, on a frozen clock (`tick`), so a
   frame is a function of the second and two runs agree. Do not `wait-frame` text the change
   covers: the title hid the hint row and the scene exited 1 on a timeout with every snap taken.
   A key may end the thing under test; a scene that needs to keep going sends none.
2. **Fast pass, headless, every edit.** `cargo build -q -p yi-cli`, `tui_see.py run`, read the
   `frames/snap-*.txt`. Sixteen colours, no kitty images, seconds a round. Settle shape, timing
   and what is covered here.
3. **Look pass, real Ghostty, once per round.** `just yi-term` once, then
   `tui_see.py run <scene> --host term --video --label ghostty` and Read
   `target/see/<scene>/ghostty/term/snap-*.png`. This is the only pass that shows true colour,
   kitty images and what draws above text. Run `just yi-term` and the term host with the real
   `HOME`: under `scripts/wt_home.sh` docker finds no context and says the daemon is down.
4. **Never watch the user's Ghostty yourself.** The computer-use grant for a terminal is click
   tier, and a window launched with `open -na` opens on another Space. The recorder is the eye.
5. **Timeline on a true terminal when images are involved.** Start `yi serve --socket S`, fork
   `yi console --socket S` under a pty with `TERM=xterm-kitty` (answer `ESC[6n` with
   `ESC[34;1R`), stamp every read chunk, and find the first `a=p` for each image id. That is how
   "no second logo" became a number: avatars at 3.03 s and the orb at 3.16 s of a 3.0 s
   animation. `scripts/tui_pty.py` hangs here: the daemon `ensure_daemon` spawns inherits the pty.

## What made it look right

- **Fullscreen is every cell of the frame, drawn last.** Under the chat is a pane effect; the
  owner asked for the whole TUI three times. Draw over the rail, tabs, composer and hint row.
- **Images draw above text.** Cover the screen and every kitty image (avatars, orb, notebook)
  still shows through unless it is unplaced for the duration, then placed again.
- **One logo at a time, and the ending is a design element.** Land the big mark on the small one:
  take its cells from the engine's own geometry (`yi_orb::mark_at`), not from a screenshot, and
  give the screen back by a seeded dissolve around it. A pop is the failure.
- **Short.** 5.4 s read as long; 3 s did not. Keep the phases in one list of seconds and scale
  them together. Any key ends it, and ending is one state flag plus one last redraw: a surface
  that stops asking for frames leaves its last frame on screen.
- **Fixed contrast, supersampled cells.** Normalise a field by percentiles taken once from
  sample frames, never per frame (it pumps), and average 3x3 samples a cell, or a fractal is
  noise. Leave the sparkle that is the structure.
- **Judge at real size.** 80x24 hides what a 200x50 window shows; check both.

## Cost

- Time a frame in the debug build at 110x32 and 200x50 before the first look pass: 7 ms and 18 ms
  was the Kali fold with 3x3 samples. Past about 30 ms the console stutters.
- Ask for a frame once per interval on the app's own clock, never once per loop turn: the first
  version drew at 58 fps, wrote 590 KB a second, and was fixed to 17 fps and a third of the bytes.
- The dist binary has a hard cap and a baseline; a pretty change that grows it by 16 KB needs the
  cap decision first (`just push` runs the ratchet and refuses over the cap).

## Proof for the PR

A sentinel test (pre-fill the screen with `X`, run the cover, count survivors), landing cells
derived by hand from the stroke constants, a mutant for each behaviour seen red, one real-binary
journey in `crates/cli/tests/console_drive.rs`, a ledger row naming it, and the recorder's stills
and `run.webp` for the owner. `echo` a log path from a background shell with the variable set
in that same command: an unset `$SP` wrote to `/check.log` on a read-only root and a ten-minute
gate never ran.
