# TUI Visual Upgrade — diffs in the transcript, a typed ipython cell, highlight, and the subagent surfaces

```
status:  PROPOSED 2026-08-28. Nothing landed. Steps carry their own D-rows and
         version bumps when they land; this document authorizes none of them.
         D-row and U-row numbers below are placeholders — claim real numbers by
         reading ARCHITECTURE.md's header and last D-row immediately before
         writing (0.35.0/D55 and 0.38.0/D57 were both lost to collisions).
date:    2026-08-28
sources: four-scout study of the §8.14 donors (2026-08-28): the reference
         <ref>/tui/src (diff_render.rs, exec_cell/, shimmer.rs, motion.rs,
         render/highlight.rs) · the reference packages/coding-agent (edit/renderer.ts,
         modes/components/diff.ts, tools/bash.ts, tools/todo.ts,
         modes/theme/shimmer.ts) · the reference packages/tui
         (routes/session/index.tsx, util/collapse-tool-output.ts) ·
         the reference packages/coding-agent/src/modes (ipython-cell.ts,
         subagent-summary-line.ts, agents-view-*.ts, theme/working-icon.ts,
         components/diff.ts) — the reference's TUI is currently under an A.2
         excise line; step P0.d amends it before any port · Yi ground truth:
         crates/tui/src/{cell,app,colors,approval,hud,markdown}.rs,
         crates/tools/src/{diff,ipython,builtins}.rs, hashline/tool.rs,
         crates/types/src/{event,kernel}.rs · YI_DESIGN §8.14 U-table, §13.3,
         §13.5 · TODOS A7 · D41/D43/D47
```

## 1. Thesis

The transcript is honest but mute. An edit renders as one digest line while a
real `GitPatch` is computed and dropped two layers below; an ipython cell that
ran 40 lines of Python, wrote three files and raised renders identically to a
`glob`; nothing anywhere is syntax-highlighted; subagents are two-line task
cells with no aggregate view. Every donor Yi already cites solves these with
mechanisms that fit Yi's inline-scrollback skeleton — none of this needs a new
rendering medium, so no visual-approach gate applies beyond the mocks noted
in P5 (the only new surface shapes).

Eighteen items, seven phases, dependency-ordered. Phases are independently
landable; each is its own version bump + changelog row. Within a phase, steps
are single commits.

| phase | items | what |
|---|---|---|
| P0 | — | data plumbing + doc amendments everything else rides on |
| P1 | 1, 2, 3 | transcript diffs |
| P2 | 4 (5 stretch) | typed ipython cell |
| P3 | 6 | `highlight` feature (TODOS A7) |
| P4 | 7, 8, 9, 10 | tool cell polish |
| P5 | 11, 12, 13, 14 | subagent surfaces |
| P6 | 15, 16, 17, 18 | motion kit |

P1/P2 land before P3 and take a `highlight: Option<&Highlighter>` hook from
day one so the renderer's layout is not re-litigated when color arrives.
P4–P6 are order-free after P0.

## 2. Ground truth (verified 2026-08-28)

Data that already exists — the headline discovery is that P1 and P2 are
almost pure TUI work:

- `crates/tools/src/diff.rs` — `GitPatch` computed per edit
  (`hashline/tool.rs:228`) and per write (`builtins.rs:54`); consumed by the
  permission ask and ACP `diff.patch`, discarded before the transcript.
- `yi_types::event::ToolResult.details: Value` (`event.rs:82`) — free-form,
  already persisted, already delivered to the TUI on `ToolExecutionEnd`.
  bash already sets `details.exitCode` (`builtins.rs:480`).
- `yi_types::kernel::ExecuteResult` — stdout / stderr / result / `diffs:
  Vec<KernelDiffDisplay>` / `attachments` / `error: KernelError {ename,
  evalue, traceback}` / `status` / `duration_ms`. The ipython tool flattens
  all of it to text and forwards only counts in details (`ipython.rs:61-97`).
- `AgentEvent::{PermissionRequested, PermissionResolved}` carry
  `tool_call_id` (`event.rs:144-150`) — item 7 needs no wire change.
- The cell source for a kernel call is `ToolExecutionStart.args["code"]` —
  item 4 and item 12 need no wire change for the code itself.
- `crates/tui/src/colors.rs` — `ColorTier::{TrueColor, Ansi256, Ansi16}` +
  dark/light detection (U17) — the exact fork the diff palette needs.
- TUI is 8,537 lines against D43's ≤ 10,000 (see §6 size budget).

## 3. Cross-cutting decisions (settle once, cite from every step)

**3.1 Expansion is the transcript mode, never per-cell state.** Donors expand
per cell (click / Ctrl+O on a focused row). Yi has no mouse and no cell
focus; U32's Normal/Thinking/Verbose cycle is the expansion mechanism for
everything in this plan. "Collapsed" below means Normal, "expanded" means
Verbose. No per-cell toggle state is introduced; the footer hint on a
collapsed body names Ctrl+O (mode cycle), which repaints retroactively via
the D45 rebuild.

**3.2 The diff row grammar** (one renderer, three consumers: edit/write cells,
ipython `diffs`, approval view later):

```
 313 │ context           ← gutter: right-aligned, min 3 digits, `│` terminator
+322 │ added             ← sign fused into the gutter column (the reference shape)
     │ wrapped continua… ← blank gutter, sign column preserved as space
   ⋮                     ← hunk separator: gutter-width spaces + dim ⋮
```

- Gutter minimum 3 digits is load-bearing, not cosmetic: a streaming diff
  crossing line 100 must not re-pad rows already committed to native
  scrollback. Incident comment required.
- Duplicate gutter numbers blanked: `-N` directly followed by `+N` renders
  the second gutter as spaces.
- Wrap, don't truncate, with a blank-gutter continuation that keeps the sign
  column. This deliberately diverges from
  U12's truncate-only rule: U12 protects a fixed-height approval box; the
  transcript has no height budget and a truncated edit body hides the change
  the reader opened Verbose to see. U12's approval view keeps truncation.
- Four style layers per row: gutter, sign, content,
  full-row line background.

**3.3 The diff palette**, forked on `ColorTier` + dark/light (the reference
`diff_render.rs:63-78`, ported verbatim including the values):

| tier | added | removed |
|---|---|---|
| TrueColor dark | bg `#213A2B` | bg `#4A221D` |
| TrueColor light | bg `#dafbe1`, gutter bg `#aceebb` | bg `#ffebe9`, gutter bg `#ffcecb` |
| Ansi256 dark | bg idx 22 | bg idx 52 |
| Ansi256 light | bg idx 194, gutter 157 | bg idx 224, gutter 217 |
| Ansi16 | fg green, no bg | fg red, no bg |

Context rows: default style, no line bg (terminal ground shows through).
Light-theme gutters are more saturated than the row so numbers stay legible.
Once P3 lands, highlighted deletion rows add `DIM` over the syntax color.

**3.4 Scrollback commit constraint: no animated glyph on the head row of a
multi-row body.** the reference's rule (`edit/renderer.ts:811-814`): a spinner on row 0
of a block pins the streaming-commit boundary at the top and the block cannot
scroll-append. Yi's tool head line already carries the spinner and today's
bodies are Verbose-only, so this binds only new *streaming* bodies (P2 has
none in v1 — the ipython body renders at End). Recorded now so a future
streaming body doesn't rediscover it.

**3.5 Details are additive JSON, documented in the design, never a new wire
struct.** `ToolResult.details` is deliberately `Value` (§19 flattened-extra
philosophy); each phase documents its keys in the design table row rather
than adding serde types. The kernel shapes (`ExecuteResult` etc.) already
live in yi-types and embed as JSON verbatim.

**3.6 One clock, two cadences** (P6, but the constant lands wherever first
touched): every animated glyph derives from a single process-relative
elapsed, braille at 80 ms for I/O spinners, diamond `◇◈◆◈` at 250 ms for
agent-level work. No per-cell tickers.

## 4. Phases

### P0 — plumbing + paper (no visible change)

**P0.a — edit/write patch into details.** `hashline/tool.rs` and
`builtins.rs` (write arm) put the already-computed patch on the result:
`details = {"patch": <GitPatch text>, "added": N, "removed": N}` (counts from
one pass over the patch: lines starting `+`/`-` minus headers). Also on the
*error-with-partial-apply* path where a patch exists. `preview_lines` in
`app.rs` must not double-render patch text — the TUI reads details, content
text is unchanged (schema-stable: purely additive).

**P0.b — ipython outcome into details.** `ipython.rs:90` grows the details
object: `"stdout"`, `"stderr"`, `"result"`, `"error": {ename, evalue,
traceback}` (embed `ExecuteResult` fields verbatim; keep the existing keys).
Attachments stay a count in v1 (P2 stretch revisits). The joined-text
`content` is unchanged — the model-facing view is not this plan's business.

**P0.c — TUI receives typed bodies.** `ToolCell` gains
`details: serde_json::Value` (default `Null`), populated at
`ToolExecutionStart` merge and `ToolExecutionEnd` (`app.rs:570/617/720/1157`
sites). Zero rendering change in this step.

**P0.d — Appendix A amendment (its own commit, before any port).**
the reference's `packages/tui/` and `modes/` sit in the A.2 excise block; the
2026-08-28 study read `modes/` at the user's direction. Mirror the reference
precedent (A.8's excise note): narrow the excise line, add a cited-span table
under A.2 for exactly what P2/P5/P6 port — `interactive/components/
ipython-cell.ts` (head-line contract, traceback split), `core/tools/
code-preview.ts` (scored preview), `interactive/components/
subagent-summary-line.ts:83-140` (tray), `agents-view/agents-view-state.ts`
(row model, heartbeat aggregation — read-only reference), `interactive/
components/context-tree-format.ts:112-200` (token tree), `theme/
working-icon.ts` (pulse cadence), `components/diff.ts` + `theme/theme.ts:
874-890` (rich diff, highlight laziness — read-only reference). Everything
else in the reference TUI stays excised. Same commit updates §8.14's donor
list to name the reference.

Tests: P0.a/b assert details keys on a real tool execution (existing tool
test files); golden session fixtures untouched (additive). No baseline moves.

### P1 — transcript diffs (items 1–3)

**P1.a — `crates/tui/src/diffview.rs`** (new file; cell.rs is at 569 lines
and this is a seam, not a line-count split): parse `GitPatch` text (headers,
`@@` hunks — the format is Yi's own `diff.rs` output, stable), render per
§3.2/§3.3. Public surface: `render_diff(patch: &str, width, theme, budget:
DiffBudget, highlight: Option<&Highlighter>) -> Vec<Line>` — pure, snapshot-
testable. `DiffBudget { hunks: 8, lines: 40 }` for Normal, unbounded for
Verbose, plus a 10,000-line hard safety cap either way.

**P1.b — collapse semantics** (the reference `truncateDiffByHunk`, adapted): change
lines get priority, remaining budget distributed across context segments,
a context run sandwiched between kept hunks splits head/tail around a `…`
gap row; footer `… (3 more hunks, 22 more lines) · ctrl+o` dim. Blank/gap
rows collapse to one dim `…`.

**P1.c — word-level highlight** (item 2): when a hunk is exactly one removed
+ one added line, split both on whitespace/word boundaries (hand-rolled ~40
lines, no dep), mark changed tokens on the added row with `REVERSED`,
skipping leading indentation. Applies at every tier
(reverse video survives Ansi16).

**P1.d — the edit/write cell body.** `ToolCell::lines`: when
`details["patch"]` is present, Normal mode renders head + digest + the
budgeted diff body (this is the item-1 headline: the diff shows *without*
Verbose); Verbose renders it unbudgeted. Head digest gains colored stats:
`└ path +12 -3` with `+12` in success, `-3` in error color (the reference's
`edit-summary.ts` shape on Yi's existing `└` slot). The old numbered-line
body for `edit` is deleted (subsumed).

Docs: one D-row — transcript tool cells grow typed diff bodies in Normal
mode, revising U15's head+digest-only contract; U15 amended, new U-row for
the diff renderer citing the reference/the reference spans; feature-ledger TUI row notes;
`details.patch` keys documented on T13's row. Changelog + version bump.

Tests (doctrine: name the regression a consumer sees):
- insta snapshots of `render_diff` at each tier × dark/light — asserts the
  palette fork and gutter shapes byte-exactly (a consumer parses columns).
- min-3-gutter: diff crossing line 99→100 renders identical gutter width for
  early rows (the scrollback-repad regression).
- duplicate-blanking, `⋮` separator, wrap-continuation sign column.
- word-diff: changed-token spans only, indentation never reversed.
- budget: change lines survive, context splits, footer counts exact.
- headless drive (`yi tui --headless`) over a faux turn whose edit result
  carries a patch — frame dump in the landing report (TUI verification law).
- Seen-red: write the Normal-mode-body test against current HEAD first and
  watch it fail (body absent), then land.

### P2 — typed ipython cell (item 4; item 5 stretch)

**P2.a — head line, byte-identical across modes** (the reference
`ipython-cell.ts:368-373`, incident comment required):

```
  ⊙ python · df = pd.read_csv("data.csv") · ↑ 12 ↓ 4 lines · 340ms
  ✗ python · train(model) · ↑ 8 lines · 4.1s · ValueError
```

Language chip `python` / `bash` / `bash · python` (a `%%bash` cell whose
scored line is Python); `↑` input lines from `args.code`, `↓` output lines
from stdout+stderr+result, the word `lines` kept to disambiguate from token
counts; duration from `details.durationMs`; failed cells append `ename`
(`ename: evalue` only when width ≥ 48 cells). Status glyph stays Yi's
spinner/`✓`/`✗` set. The head replaces `summary_of`'s generic argument echo
for `ipython` only.

**P2.b — scored code preview**, ported adapted from the reference
`core/tools/code-preview.ts` into `crates/tui/src/pycell.rs` (new file, pure
functions): skip comments / imports / `set -e` / decorators / low-signal
calls (`print`, `len`), prefer effect calls (`write_text`, `mkdir`,
`execute`), collapse whitespace, redact base64 runs → `<blob>` and
`sk-…`/token/key/secret/password assignments → `<redacted>`, cap 64 chars.
Redaction is a trust-boundary behavior — never simplified away.

**P2.c — Verbose body**, in order: source under a `› ` first-line gutter with
two-space continuation (dim), each line through the P3 hook when available
(bash cells flat); stdout indented 2 under the gutter in text color; stderr
same indent, muted; `result` as stdout; traceback split — locate `Traceback
(most recent call last):` or `<Ename>:` in the raw stream so preceding
stdout renders as output, traceback body in error color (the reference
`splitTraceback :301-320`; Yi has structured `details.error.traceback`, so
the heuristic only serves mixed stdout) ; `[kernel restarted]` as a warning
row; `diffs` (kernel-side file edits) rendered by computing
`diff::patch(old_str, new_str)`… no — `KernelDiffDisplay` carries
`old_str`/`new_str` already: feed both through P1's renderer with a
per-diff `╰─ path` header row. Failure bodies are never mode-gated (existing
U15 law): a failed cell shows source + traceback in Normal too.

**P2.d (stretch, own change) — kitty images.** `KernelAttachment.path` is
optional; when the kernel wrote the attachment to the artifacts dir, put
`attachmentPaths` in details (P0.b addendum), and on the kitty path (U33's
gate: TERM kitty/ghostty or `KITTY_WINDOW_ID`) transmit the image after the
cell, zlib-deflated like U34; other terminals render the count row
`2 images (artifacts/…)`. PTY-harness verification with `--term
xterm-kitty`, counting emitted APC escapes (the capability-path law —
a clean exit proves nothing).

Docs: one D-row (typed ipython cell revising U15 for the kernel tool; head-
line byte-stability as the recorded invariant), new U-row citing the
the reference spans from P0.d; ledger row for the kernel feature updated.

Tests: head-line byte-equality Normal vs Verbose (the layout-shift
regression); scored-preview table tests incl. redaction cases (secret in
code must never reach a frame — name that failure); traceback split with
mixed stdout; a failed cell shows traceback in Normal; insta snapshots;
headless drive over a faux ipython turn; seen-red on the head-equality test.

### P3 — `highlight` feature (item 6, TODOS A7)

**P3.a — dependency, the §13 gauntlet in one commit:** §13.3 row for
`syntect` (`default-features = false`, `features = ["parsing",
"regex-fancy", "dump-load"]` — §13.5 bans onig and two-face explicitly, so
fancy-regex is the engine; alternative considered: hand-rolled per-language
lexers, rejected as unbounded maintenance for ~20 languages), deny.toml
entries, docs/size-ledger.md with measured dist-binary + startup delta
(dist profile, never release), transitive count re-checked against ≤ 135.
Grammar set is a curated ~20-language `SyntaxSet` compiled to a committed
binary dump via a build-time script under scripts/ (the .sublime-syntax
sources are not vendored into src/); languages: rust, python, ts/js, json,
toml-not-banned-as-format, yaml, md, sh, c, cpp, go, html, css, sql, diff.
Budget check may force the set smaller — the ledger records what made the
cut. Cargo feature `highlight` on yi-tui only, in the §13.4 allowlist
(check_manifests.py) — the one place a feature is legal to add.

**P3.b — scope→theme mapping, no tmTheme.** Do not load syntect themes:
map returned scopes onto Yi's `Theme` through a fixed ~10-entry table
(comment→dim, keyword/type/function/string/number/variable→theme colors, the
the reference nine-color shape), keeping the reference's conversion law: fg + BOLD only, never
bg/italic/underline. Adaptive by the existing dark/light detection. This
kills the plist/yaml-theme transitive tail and makes every tier degrade
through Theme's own machinery.

**P3.c — the cache is load-bearing.** LRU 256 keyed `(lang, hash(code))`,
cleared on theme change; the reference measured 26–40 ms per 100-line re-tokenize —
uncached, the 80 ms spinner starves. Incident comment carries the number.
Caps ported verbatim: 512 KB / 10,000 lines / 4 KiB per line pre-scanned
before highlighting is attempted.

**P3.d — application points**, each a small commit: markdown fences (rail
already carries the language label — `markdown.rs:355-373`); diff context +
bodies per-hunk-as-one-block so parser state survives multi-line strings,
DIM on deletions; bash command in the tool
head (P4.c); read/edit Verbose bodies; ipython source (P2 hook).

Docs: A7 closed; §8.14's "no syntax highlighting at launch" sentence updated
to name the landed shape (the design already reserves the feature — this is
scheduling, but the scope-table-not-tmTheme choice is a D-row); ledger.

Tests: token-to-style table tests over canned source (no source-grepping —
feed strings, assert spans); cache-hit test (second call allocates nothing /
returns identical spans); cap tests (oversized input → plain); fence + diff
snapshots re-recorded under the feature; build with and without `highlight`
in CI (`just check` covers default; one cargo invocation with the feature).

### P4 — tool cell polish (items 7–10)

**P4.a — amber awaiting-permission (7).** App tracks
`pending_permission: HashSet<tool_call_id>` from `PermissionRequested` /
`PermissionResolved`; a `ToolCell` in that set renders head + summary in
warning color (the reference's precedence: warning beats running-text color).
Denied keeps the existing strikethrough. Test: event-sequence table test —
Requested colors the row, Resolved un-colors, Denied strikes.

**P4.b — Explored group (8).** Extend the existing consecutive-read
coalescing (`app.rs`) to the read/grep/glob/find class: one cell headed
`✱ Explored` (spinner while any member runs), body rows `verb argument ·
digest` with the verb in accent — the reference's cyan verb column
(`exec_cell/render.rs:293-385`). Group caps at 32; a failed member breaks
the group so the failure renders alone with its body (failure never buried
in a group). Digest logic per member unchanged.

**P4.c — bash cell (9).** Summary becomes `$ <command>` (highlighted under
P3, dim `$`), replacing the generic `glyph name argument` echo for bash
only; digest gains a stats tail from details: `└ <first output line> ·
exit 1 · 1.2s` with nonzero exit in error color (`details.exitCode` exists;
elapsed already on the head — move it into the stats tail for bash to avoid
double time). the reference's bracket dress `⟦…⟧` is skipped: Yi's `·` separators
already carry metadata, one grammar is enough. (ponytail: reuse.)

**P4.d — sibling spacing (10).** Pure layout rule in the transcript
assembler: a blank line precedes a cell iff the previous cell rendered
multi-line or this cell will; runs of one-liners pack flush (the reference's
pre-layout margin, `layout.ts:8-25`). Task cells drop their unconditional
forced blanks in favor of the rule. Because commits are append-only to
scrollback, compute from the *already-rendered* height of the previous cell
— the exact information Yi's commit path has. vt100 test at a nonzero
viewport offset (the law), plus a snapshot locking a mixed run: three
one-line tools flush, wrapped grep followed by a blank.

Docs: one D-row covering the four as "U15/U27 polish" with the spacing rule
as the recorded invariant; U15/U27 rows amended.

### P5 — subagent surfaces (items 11–14)

Visual-approach gate applies here (new surface shapes): P5 opens with a
one-screen static mock of the tray line, the grouped spawn cell, and the
agents popup, posted for sign-off before implementation. The fullscreen
the reference dashboard is explicitly **not** ported — alternate screen is
banned (§8.14); its row model and navigation port onto Yi's existing
overlay/popup primitives instead.

**P5.a — tray line (11).** The HUD's subagent section (U28, `hud.rs`)
gains the reference's counts line as its header row when children exist:
`● 2 running · ◐ 1 idle · ○ 3 done` (success/warning/dim), replacing the
plain `Subagents` header; per-child rows below unchanged, cap 8 stands.
Goal spine untouched. No focus ring in v1 — navigation stays U29's existing
child-focus keys; the tray is display-only (the composer already owns ↑/↓).

**P5.b — spawn-code grouping (12).** Children spawned by one kernel call
share that call's `tool_call_id` (`rlm.run` inside an ipython cell). Task
cells born under a live ipython tool call render grouped beneath it,
indented one level, and Verbose shows the spawning cell's source above the
group via P2's cell (the code is already the ipython cell's body — the
group is a placement rule, not new data). Requires the runtime to stamp
`ChildUpdate` (or the child-start event) with the originating
`tool_call_id` — verify the field exists; add additively if not. This is
the surface no donor has in Rust and the one most native to Yi's
kernel-spawned children.

**P5.c — `/agents` token tree (13).** A `BottomView` popup (existing U11
machinery) rendering the reference's `/context` shape:

```
   agent            tokens    cost   context
 ● parent            124k   $4.31   ▓▓▓▓▓▓░░░░ 62%
 ├─ ◆ scout           84k   $0.21   18%
 └─ ✓ builder        230k   $0.88   41%
```

Own-usage columns so they sum; 10-cell `▓/░` bar on the root only, warning
color ≥ 80%. Data: `ChildView`/`ChildUpdate` — U29's footer already shows
per-child tokens/cost; reuse that source. Slash command `/agents` joins
`SLASH_COMMANDS` (A5 pattern: one handler arm).

**P5.d — kill confirm (14).** In the `/agents` popup, `x` on a child row
arms a 2-second in-row confirm — the row's right column swaps to red
`x again to stop` (the reference's `DELETE_CONFIRM_DURATION_MS` pattern);
second press within the window interrupts via the existing mailbox
`rlm.interrupt` path; timer lapse restores the row. Keymap rows via U8.

Docs: one D-row (subagent presentation set, revising U28's header and
adding the popup; records the no-fullscreen decision and the spawn-group
placement rule); U28 amended, new U-row for the popup; TODOS F3's "TUI
half" row notes what this does and does not cover (composer-on-child stays
open); ledger.

Tests: HUD counts table test over `cards(&AgentState)` (pure); popup
snapshot with three children incl. sum row; confirm-window state test
(arm → lapse → restore, arm → press → interrupt command emitted); headless
drive script focusing and killing a faux child; vt100 nonzero-offset for
the tray (it lives in the live region).

### P6 — motion kit (items 15–18)

All pure per-frame math + one clock; the faux provider answers instantly, so
every step here tests state transitions and frame functions directly, never
end-to-end animation (the standing TUI law).

**P6.a — unified pulse (15).** One process-relative clock already drives the
spinner phase; derive both cadences from it (§3.6): braille 80 ms for tool
cells, diamond `◇◈◆◈` 250 ms for task cells, HUD child rows, and the status
subagent badge. Delete any per-surface phase counters. Test: same instant →
same glyph across surfaces (the lockstep contract).

**P6.b — shimmer working line (16).** the reference `shimmer.rs` ported adapted
(~80 lines): raised-cosine band (half-width 5, padding 10, 2 s sweep),
phase from the process clock, color `blend(bg→fg, t·0.9)+BOLD` at TrueColor
— blending toward the terminal's own colors so it reads on any theme; tiers
degrade `t` to DIM/normal/BOLD at Ansi256/16. Applies to
the spinner-line narration (U16's `i` intent text). Frame timer already
exists while a spinner is visible; clamp step to one frame (U34's law —
the unclamped orb consumed a whole animation in frame one).

**P6.c — strikethrough sweep (17).** HUD todo/goal rows record `done_at`;
for 12 frames after, render `partial_strike(label, ceil(len·k/12))` — SGR 9
over a left-to-right sweep, then settle into the
existing done style. Pure fn + table test over the frame ramp.

**P6.d — thinking pulse (18).** The `∴ thinking · N lines` collapsed row's
glyph cycles `✻ ✼ ❉ ❊ ✺ ✹ ✸ ✶` (all width-1 — verify with unicode-width in
a test, the fixed-width contract that prevents reflow jitter) with
raised-cosine dwell eased 70→230 ms (the reference's breath), only while the thought
is streaming; static `∴` at rest. Dwell math is a pure fn with a table
test; the streaming flag is the state transition test.

Docs: one D-row for the motion kit (records the one-clock invariant and the
reduced-motion posture: every animated glyph has a static fallback chosen at
the call site, the reference's `motion.rs` discipline — a `YI_*` env var for
reduced motion is *not* added unless a row in env_vars.json is budgeted;
v1 keys off the existing spinner-visibility gating only). Ledger note.

## 5. Dependency + guardrail ledger (what moves, in which commit)

- **syntect** (P3.a): §13.3 table row + deny.toml + size-ledger, same
  commit as Cargo.toml. Direct deps go 15→16-ish — re-read the actual count
  at land; if at the cap, the D-row must say what was considered instead.
- **Size ratchet (D43 ≤ 10k):** at 8,537 now; estimates — P1 +~450, P2
  +~450, P3 +~350, P4 +~250, P5 +~500, P6 +~250 → ~10.8k. The phase that
  crosses 10,000 lands with a D-row revising D43 (proposed: 12,000 — six
  surfaces from three donors; the ≤ 1,200-line per-file ceiling still binds,
  which is why diffview.rs and pycell.rs are files, not cell.rs growth).
  Fight for every line first: each phase deletes what it subsumes (P1 kills
  the edit numbered-line body; P4.c kills the bash generic summary arm).
- **Binary ≤ 1 MiB TUI add / 6 MiB dist:** the syntect dump is the risk;
  measured in P3.a's ledger row, dist profile. If the 20-language dump
  breaks the budget the language list shrinks — the ledger records the cut.
- **Ratchet order, every phase:** code commit lands red on baselines →
  `--update` in its own commit. Test-LOC ratchet will move at every phase
  (check_test_size.py --update). Comment volume: incident comments in
  P1/P2/P3 ride existing budget headroom; shrink-only outside yi-types.
- **No new env vars** (cap 40): nothing in this plan needs one.

## 6. Risks and open questions

1. **syntect transitive weight** (fancy-regex, bit-set, flate2 for the
   dump). If the measured tree blows ≤ 135 or the dist budget, the fallback
   is a hand-rolled 5-language lexer set (rust/python/json/bash/diff) —
   smaller blast radius, worse coverage; decide on the P3.a numbers, record
   either way in the D-row.
2. **Normal-mode diff verbosity.** Item 1 shows diff bodies un-asked. If a
   long refactor floods the transcript, the budget (8 hunks / 40 lines) is
   the knob; the D-row should note the budget is config-eligible later, not
   a config key now (X7 strictness — no speculative keys).
3. **`ChildUpdate` fields for P5.b/c** (spawn `tool_call_id`, tokens/cost):
   verified U29 shows tokens/cost, but stamp-the-spawn needs a runtime
   check; if absent it is one additive Option field in yi-types::subagent —
   schema-stable, but budget a fixture.
4. **Approval-view convergence:** U12 could adopt P1's renderer (one diff
   grammar everywhere) — deliberately out of scope; a later cleanup row.
5. **Item 5 (kitty images)** rides on artifacts-dir paths existing for
   attachments; if the kernel only inlines base64, the path plumbing is a
   kernel-side change (python/yi_runtime) and moves the runtime identity
   hash — its own change with a venv-rebuild note.

## 7. Landing discipline (unchanged law, restated because every phase hits it)

Each phase: build + focused tests → `just check` judged by exit codes → real
binary run (`./target/debug/yi ask --model faux/faux-1` and, for every
visual change, `yi tui --headless --keys <script> --frames <dir>` with the
frame dump in the report) → docs (D-row read-then-write, version bump,
changelog, U-rows, ledger, ADR per D-row) in the same change as the code →
baselines `--update` in their own follow-up commits. Never `git add -A`;
stage by name. Commit messages with backticks go through `git commit -F -`.
