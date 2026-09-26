# TUI capture and proof — frames for the agent, recordings for the humans

Landed: 0.96.0.

```
status:  LANDED 2026-08-30 at 0.86.0 (D88), all six phases. Renumbered from
         0.85.0 on the merge with `origin/main`, which had spent that number.
         r4 (review of the landed commit): three defects, each reproduced on
         the built binary. An unclamped `--deadline` panicked the process
         (`Instant + Duration` does not saturate); a paced `type` step outran
         the deadline because the clock is only read in the outer loop; and
         `--record` with `--snap` on one path silently destroyed the
         recording and exited 0. All three now carry regression tests.
         r3 (rendered, and the first look at the output): **`freeze` is out.**
         It is a text-screenshot tool, not a terminal emulator — it re-renders
         in its own font and window chrome, so the still was never 1:1: the
         composer border ran off the right edge and the glyph metrics were its
         own. `-c full`, the invocation the docs implied, exits 0 and writes a
         0-byte file. Both artifacts now go through `agg`, which is a real VT
         renderer, and `--snap` writes a one-event cast instead of a bare
         `.ansi` so the still and the motion are the same renderer on the same
         grid. The snap's row format was settled by measurement, not taste:
         newline-terminated rows render in `freeze` but scroll a 24-row frame
         off a 24-row screen, and the still came back blank.
         r1 (review): the TeeBackend/CrosstermBackend recording path died — it
         hung on headless tty reads, flooded the cast with per-tick cursor
         writes, and split UTF-8 at Write boundaries. Recording was respecified
         onto a hand-rolled emitter.
         r2 (implementation): **the tee came back, with the three defects fixed
         at the source, and the hand-rolled emitter died instead.** Reads
         delegate to `TestBackend` alone, cursor calls record only on a
         transition, an empty diff records nothing, and UTF-8 is reassembled at
         the flush boundary. What r1 could not see: a hand-rolled emitter owes
         correct escapes for scroll regions, clears, `append_lines` and SGR
         state — the entire `CrosstermBackend` write path, reimplemented and
         then debugged against `TestBackend`'s semantics. Reusing crossterm is
         both the smaller diff and the one already under test.
         r2 also found the real event-spam mechanism, which no review caught:
         not per-tick calls but **per-write fragmentation**. Crossterm formats
         its commands into the writer piecewise, so one event per `write` made
         a three-frame run 431 events of `─`, `\x1b[`, `m`. Moving the event
         boundary to the flush made the same run 7 events and 1.4 KB, and
         deleted the pending-raw machinery r2 had built to hold no-op writes.
date:    2026-08-30
sources: asciicast v2 spec (https://docs.asciinema.org/manual/asciicast/v2/,
         https://github.com/asciinema/asciinema/blob/v2.0.0/doc/asciicast-v2.md) ·
         agg — asciinema gif generator (https://github.com/asciinema/agg,
         https://docs.asciinema.org/manual/agg/usage/) ·
         charmbracelet/freeze — ANSI → PNG/SVG (https://github.com/charmbracelet/freeze) ·
         vt100 0.15 (existing dev-dependency; tests/common/mod.rs) ·
         crates/tui/src/drive.rs · crates/tui/src/terminal.rs ·
         crates/cli/src/main.rs headless wiring ·
         docs/plans/2026-08-29-governance.md (ratchet discipline)
```

## 1. Thesis

Yi developing Yi needs two capture channels with different consumers, and they
are the same draw stream with different sinks:

1. **Agent-facing frames** — the model reads text grids, asserts on them, and
   iterates. This channel already ships: `yi --headless --keys <script>
   --frames <dir>` runs the real App reduce/draw loop over a `TestBackend`
   and dumps deduplicated `NNNN.txt` frames (drive.rs `run_headless`).
2. **Human-facing proof** — a PR that changes the TUI attaches a GIF or PNG so
   a reviewer can judge visual appeal, which the model is unreliable at. This
   channel does not exist.

The design principle: **capture once at the draw boundary, render never.**
Yi emits only two portable text artifacts — an asciicast v2 `.cast` file and a
raw-ANSI final frame — and all pixel rendering happens in external, dev-machine
tools (`agg` for GIF, `freeze` for PNG). No image encoder, no font rasterizer,
no new dependency enters the binary. The dependency count, tool-table bytes,
and dist size ratchets are untouched by construction; only the src LOC ratchet
moves, deliberately.

One writer serves both artifacts, and it is ratatui's own: `RecordingBackend`
tees `TestBackend` (assertions) and `CrosstermBackend` (recording), and
`buffer_to_ansi` runs a whole frame through a second `CrosstermBackend` over a
byte sink. Escape generation for scroll regions, clears and SGR state is code
already under test rather than a second implementation of it, and the round
trip is pinned by replaying the cast through `vt100` against the frames the
agent asserted on.

## 2. What exists today

- `drive.rs`: script grammar (`key`, `type`, `wait`, `wait-idle`,
  `wait-frame [!]text`, `quit`), `HeadlessBackend(TestBackend)`, the real
  App/runtime bridge, frame dedup + dump to `frames_dir/NNNN.txt`, 60 s wall
  deadline. Wired from `crates/cli/src/main.rs` behind `--headless`.
- `terminal.rs`: `draw()` diffs double buffers and calls `Backend::draw` with
  only the changed cells — but calls `hide_cursor()` and `flush()` on
  **every** invocation, and the drive loop draws every ~2 ms tick. Any
  recorder at this boundary must therefore emit events from *state changes*,
  never from *calls*.
- `tests/common/mod.rs`: a `CrosstermBackend`-over-`vt100::Parser` mock used
  by `tui_e2e.rs`. `vt100` is a **dev-dependency only** and stays one.
- `serde_json` is already a runtime dependency of `yi-tui`.

Gap analysis: frames are unstyled text (TestBackend `Display` drops SGR), no
recording, no styled still frame, `type` is instantaneous (a recording of it
would be unwatchable), the 60 s deadline forbids paced demo scripts, and
nothing turns a drive run into a PR-attachable artifact.

## 3. Architecture

```
                    ┌────────────────────────────────────────────┐
   drive script ──▶ │ run_headless: real App + reduce/draw loop  │
                    └───────────────┬────────────────────────────┘
                                    │ Backend::draw(changed cells)
                          ┌─────────┴──────────┐
                          │  RecordingBackend  │
                          └─────────┬──────────┘
        ┌───────────────────────────┼───────────────────────────┐
   TestBackend            CrosstermBackend<CastWriter>   buffer_to_ansi(final)
   wait-frame asserts     one event per flush                 .ansi snap
   NNNN.txt frames        .cast                                  [human]
        [agent]              [human]                               │
                                │ external, dev-only               │
                          agg → .gif                        freeze → .png
```

- `RecordingBackend` wraps `TestBackend`. Every read (`size`, `window_size`,
  cursor position) delegates to the inner `TestBackend` **only** — nothing in
  the capture path ever touches a real tty, so headless CI cannot hang.
- In `draw`, the changed cells are collected once, applied to the inner
  backend, and replayed to the crossterm side. An empty diff stops there:
  crossterm ends even an empty `draw` with a reset triple. Cursor hide/show
  records only on a transition, because `terminal.rs` hides the cursor after
  every draw.
- The `Write` path (OSC title, prompt zones, the synchronized bracket)
  forwards to the recorder as well.
- `CastWriter` accumulates bytes and emits **one event per flush**, which is
  one `Terminal::draw`. It reassembles UTF-8 there, so a character split
  across writes can never corrupt a JSON line, and it strips the
  synchronized-update bracket — a live-display hint that would otherwise put
  an event on every idle tick and can strand a player mid-update when its
  closing half falls in a frame that emitted nothing.

## 4. Phases

### Phase 1 — CastWriter + RecordingBackend (`crates/tui/src/capture.rs`)

New module, 319 LOC as landed:

- `CastWriter { out: BufWriter<File>, start: Instant, buf: Vec<u8> }`
  implementing `Write`: `create` writes the asciicast v2 header
  (`{"version": 2, "width": W, "height": H, "env": {"TERM":
  "xterm-256color"}}`), `write` accumulates, and `flush` emits one
  `[<secs.f6>, "o", payload]` line via serde_json (never hand-escaped),
  keeping any incomplete trailing character for the next frame.
  Newline-delimited JSON means a crashed run still leaves a playable prefix.
- `RecordingBackend { inner: TestBackend, cast:
  Option<CrosstermBackend<CastWriter>>, cursor_hidden: bool }` implementing
  `Backend` (reads from `inner`; mutations applied to `inner` and mirrored to
  `cast`) and `Write`. Replaces `HeadlessBackend`; with `cast: None` it
  degenerates to the old behaviour at the old cost.
- `force_color_output(true)` before either crossterm backend exists —
  crossterm strips colour when its writer is not a tty, which would make
  every recording monochrome.
- **Error discipline (HAR):** every `CastWriter` and recorder IO failure
  propagates as `io::Result` up through `Backend::draw` into `run_headless`,
  which prints the error and exits nonzero. A silently truncated proof is
  worse than no proof. The existing `fs::write(...).is_ok()` swallow at
  drive.rs:327 is the anti-pattern; the frame-dump path is fixed to match
  while the function is open.
- `DriveOptions` gains `record: Option<PathBuf>`; CLI gains `--record
  <file.cast>`, rejected with a usage error when `--headless` is absent —
  never silently ignored.

Timestamps come from `Instant` elapsed — wall-clock noise in `wait` steps is
exactly what makes the replay watchable, so determinism is explicitly not a
goal for the `.cast` sink (it remains a goal for the `.txt` sink, unchanged).

### Phase 2 — Final frame as a still (`--snap <file.cast>`)

- `buffer_to_ansi(&Buffer) -> io::Result<Vec<u8>>` feeds every cell of a
  frame to a `CrosstermBackend` over a shared byte sink, opening with
  `\x1b[0m\x1b[H` so the paint stands alone. Cells are **placed by absolute
  cursor move, never newline-terminated** — a terminal is the reader. The
  sink is shared rather than reclaimed because ratatui 0.29 gates
  `CrosstermBackend::writer` behind an unstable feature.
- `--snap` writes it once at loop exit, wrapped by `write_still` as a
  one-event cast so `agg` renders it.
- Rendered on the dev machine by freeze into the PNG still. The exact freeze
  invocation for raw-ANSI input (stdin vs file, `--language ansi` or
  autodetect) is **verified against the installed tool during Phase 4**, not
  assumed from memory.
- Optional later: `--frames-style` emitting `NNNN.ansi` beside every
  `NNNN.txt` if the agent proves to need color assertions; not built now.

### Phase 3 — Watchable timing

- New script step `type-ms <n>` sets per-character delay for subsequent
  `type` steps (default 0, so existing scripts and CI stay instant).
- New flag `--deadline <secs>` (default 60, unchanged) — a paced proof
  script with `type-ms 35` plus `wait` beats can legitimately exceed the
  current hard-coded wall limit.
- Playback speed and idle compression beyond that are agg's job (`--speed`,
  `--idle-time-limit`), not Yi's.

### Phase 4 — `just tui-proof <script>`

justfile target, no Rust:

1. `cargo run -- --headless --keys <script> --frames <out>/frames --record
   <out>/run.cast --snap <out>/final.ansi` (dev profile; proof generation
   never happens on user machines).
2. `agg --theme monokai --font-size 16 --idle-time-limit 1 <out>/run.cast
   <out>/run.gif` (theme and font size pinned so proofs look uniform across
   machines).
3. `agg --theme monokai --font-size 16 <out>/still.cast <out>/still.gif` —
   the same renderer, so the still is the motion's last frame.
4. Every render is size-checked: a renderer that exits 0 having written
   nothing printed a path and looked like it worked.
4. Print artifact paths; missing renderers degrade to a note naming the brew
   formulae (`brew install agg charmbracelet/tap/freeze`).

The human drags `run.gif` or `still.gif` into the PR. GitHub renders GIF in
comments; uploaded SVG it does not — which is why Yi never grows an SVG
renderer.

### Phase 5 — Verification

- **Round-trip e2e** (the load-bearing test, `tui_e2e.rs`): drive a scripted
  session with `--record` and `--frames`; parse the `.cast` lines with
  serde_json and feed the event payloads through `vt100::Parser` (existing
  dev-dependency, same harness as `tui_e2e.rs`); assert the parsed final
  screen equals the last `NNNN.txt` frame, that timestamps are monotonic,
  and that the event count stays proportional to the changed frames. One
  test covers the recorder, the writer and the file shape — the two sinks
  cannot drift silently.
- **Style assertions** for `buffer_to_ansi` (`tui_unit.rs`): a styled buffer
  in, and the dump must carry the origin anchor, the text, the bold
  attribute and both indexed foreground colours. `insta` was specified here
  and dropped: the repo has no snapshot files, and a plain assertion names
  what must survive instead of freezing bytes nobody will re-read.
- **UTF-8 split** (`tui_unit.rs`): a box-drawing character written two bytes
  before a flush and one byte after must reassemble across the two events,
  and a byte that can never complete must be dropped rather than held.
- **Idle filter** (`tui_unit.rs`): 200 ticks of exactly what `render::draw`
  does when nothing changed — sync bracket, empty diff, `hide_cursor`, flush
  — must add nothing after the first tick's genuine cursor hide, and the
  next real cell must still land. This is the deterministic guard the
  round-trip's proportional event count only approximates.
- **Still-equals-motion** (in the round-trip test): the still cast has
  exactly one event, and replaying it cold into an empty parser must land on
  the same screen the recording ends on. This is the 1:1 property stated as
  an assertion rather than an intention.
- agg is never a CI dependency; CI verifies the cast and frame artifacts
  only. The rendering itself was verified by hand — `run.gif` at 790×560
  over 16 frames and `still.gif` at 790×560 — and the images were looked at,
  which is how the `freeze` problems were found at all.

### Phase 6 — Docs, skill, governance

- Update the `yi-tui-verify` skill: add the record/snap/proof flow beside the
  existing drive-mode instructions, so future sessions reach for `just
  tui-proof` instead of reinventing capture.
- `docs/ARCHITECTURE.md` decision-log entry (D88): capture-at-draw-boundary,
  render-externally, crossterm-over-a-hand-rolled-emitter, and why vt100
  stays a dev-dependency.
- PR convention (governance addendum): a PR whose diff touches
  `crates/tui/src` rendering attaches at least one proof artifact.
- Ratchet accounting, as landed. Estimates are kept beside the measured
  numbers because two of the misses are the design changing under the plan,
  not an arithmetic slip:

  | Change | File | Est. | Actual |
  | --- | --- | ---: | ---: |
  | `CastWriter`, `RecordingBackend`, `SharedBuf`, `buffer_to_ansi` | capture.rs (new) | +250 | **+333** |
  | Delete `HeadlessBackend`, wire the recorder, `type-ms`, `--snap`, deadline, error propagation | drive.rs | −53 | **−35** |
  | Module export | lib.rs | +1 | **+1** |
  | Three flags plus the drive-only-needs-`--headless` guard | cli/main.rs | +30 | **+31** |
  | Untracked drift in the 0.84.0 baseline | — | 0 | **+89** |
  | **Net src** | | **+262** | **+436** |
  | Round-trip e2e (cast → serde_json → vt100 → frame, monotonicity, event count) | tui_e2e.rs | +90 | **+120** |
  | `buffer_to_ansi` style, UTF-8 split, idle-filter | tui_unit.rs | +30 | **+141** |
  | `tui-proof` recipe and `scripts/proof/demo.drive` | justfile, scripts | +25 | **+43** |

  Deps 18 direct / 166 transitive, unchanged. Tool table unchanged. Dist
  binary 5029712 → 5062864, **+33,152 bytes**, logged in `docs/size-ledger.md`
  with the two alternatives measured against it: boxing the writer built
  byte-identical (LTO had already merged the monomorphizations), and
  feature-gating `capture.rs` out of `dist` was declined at 32 KB against
  176 KB of headroom. Comment volume 1939 → 1987 and `yi-tui` 9759 → 9954
  ratchet with the landing; the 3-line comment cap held without an exception.

## 5. Risks and mitigations

- **Recorder drift** — the two sinks could disagree. Contained by the
  round-trip test: the cast replayed through `vt100` must equal the frame the
  assertions ran against.
- **Event spam regression** — two mechanisms, both live. The per-tick
  `hide_cursor()`/`draw()` in terminal.rs is guarded by the transition check
  and the empty-diff return; per-write fragmentation is handled by the flush
  boundary. The round-trip test asserts the event count stays proportional to
  the changed frames, which is what fails if either guard is removed.
- **Instant `type` in old scripts under `--record`** produces teleporting
  text. Accepted: proof scripts are authored with `type-ms`; assertion
  scripts do not record.
- **agg font mismatch** across dev machines makes proofs visually
  inconsistent. Mitigated by pinning theme and font size in the justfile
  recipe; not worth vendoring fonts.
- **Recording a real interactive session** (not scripted) is out of scope;
  if scripted proofs prove insufficient, `CastWriter` can tee the live
  terminal writer behind `--record` on the normal TUI path as a follow-up
  with its own plan.

## 5b. Merging with `claude/herdr-yi-tui-design-71d6df` (yi-console)

That branch lands `yi-console`, a second TUI over the serve daemon, whose
drive loop is a near-copy of this one. It was read against this change; the
overlap is real but small, and every resolution is named here so the merge is
mechanical rather than a judgement call.

**Two hard breaks, both at compile time, both wanted.**

1. `crates/console/src/lib.rs` imports `HeadlessBackend`, which this branch
   deletes, and reads the screen as `terminal.backend().0.to_string()`.
   Resolution: `RecordingBackend::new(w, h, None)` is a drop-in with the same
   cost, and `.screen()` replaces the tuple field. A test here
   (`the_recording_backend_drops_into_a_plain_ratatui_terminal`) pins that
   drop-in property through ratatui's own `Terminal`, which is what console
   constructs — Yi's custom `Terminal` is not involved. Two lines on their
   side, and console inherits `--record`/`--snap` the moment its
   `DriveOptions` grows the two fields.
2. `Step::TypeMs` makes console's `match step` non-exhaustive. That is the
   repo's own rule working (§ "adding a variant must break every match"): the
   console loop should decide whether it paces typing, and the compiler makes
   it decide.

**Three mechanical conflicts.** Their `typed_events` and `poll_condition`
extractions touch the same `Step::Type`/`WaitIdle`/`WaitFrame` arms this
branch edits; keep their helpers and pace inside the loop
(`for event in typed_events(&text) { …; if type_ms > 0 { draw; sleep } }`),
and use `.screen()` inside their `poll_condition` call. Their
`load_drive_script` extraction in `main.rs` sits directly above the
`DriveOptions` literal this branch extends. And that branch will collide on a
version number and on the ratchet baselines, exactly as `origin/main` already
did with this one: re-measure the union on the merged tree and renumber the
later row, as the 0.82.0 row establishes and as the 0.86.0 row now repeats.

**One thing the merge should fix that neither branch has.** `yi console
--headless` accepts `--record` and `--snap` and would silently ignore them,
because `yi_console::DriveOptions` has no such fields — the same defect this
branch fixed for `tui`. Console is the newest UI surface and therefore the
one most in need of proof artifacts, so it should gain the fields rather than
the guard gaining a command exception. Its drive loop also still carries the
`fs::write(..).is_ok()` swallow and the hard-coded 60 s deadline that this
branch replaced.

## 6. What died, and why

- **A hand-rolled SGR emitter** (r1's replacement for the tee, killed in r2):
  it owed correct escapes for scroll regions, clears, `append_lines` and SGR
  state — `CrosstermBackend`'s whole write path, reimplemented and then
  debugged against `TestBackend`'s semantics. r1 killed the tee for three
  real defects, but each was a few lines to fix at the source: reads
  delegate to `TestBackend`, cursor calls record on transition, UTF-8 is
  reassembled at the flush boundary. The tee is both the smaller diff and
  the one whose escape generation is already under test.
- **A pending-raw buffer holding no-op writes until a frame changed** (built
  in r2, deleted the same day): it existed to stop the synchronized-update
  bracket from emitting on every idle tick. Moving the event boundary to the
  flush and stripping the bracket at emit does the same job with neither a
  buffer nor a `frame_changed` flag, and fixes the fragmentation the pending
  buffer never addressed.
- **Pixel capture of real terminal windows** (`screencapture -l`,
  ScreenCaptureKit, xcap/scap): TCC permission is user-clicked and granted to
  the terminal host, headless CI is impossible, and the LLM reads text grids
  better than screenshots. Revisit only for emoji-width/terminal-emulator
  bugs, as a skill wrapping OS binaries, never as Yi code.
- **Embedding cua**: VM sandboxes, multi-GB images, and a second agent loop
  inside a 5.8 MB binary with a 20-dependency budget. Category error.
- **A first-class `screenshot` agent tool**: tool-table bytes are ratcheted
  and paid on every request forever, for a capability needed in ~1% of
  turns. The drive subcommand plus a skill costs zero prompt bytes.
- **vt100 as a runtime dependency**: unnecessary once recording reuses the
  snap emitter; vt100 keeps earning its keep in the test harness, where it
  verifies the recording instead of producing it.
- **In-house GIF/PNG/SVG rendering**: fonts, rasterization, and encoders are
  exactly the dependencies the size ledger exists to keep out. agg is
  maintained, external, and dev-only.
- **`freeze` for the still** (r3): a text-screenshot tool renders text in its
  own font and chrome, which is not the cell grid the TUI painted — the
  composer border ran off the edge, and `-c full` wrote a 0-byte file while
  exiting 0. A still must come from a terminal emulator, which is what makes
  it comparable to the motion GIF beside it. Cost of the removal: one fewer
  tool to install, and PNG becomes GIF (GitHub renders both).
- **A third `.cast` header/shape unit test** (r1): the round-trip test
  already parses every line; a separate shape test would assert the same
  bytes twice.
- **vhs**: its tape scripts duplicate drive.rs with less integration;
  adopting it would mean two script grammars for one loop.
