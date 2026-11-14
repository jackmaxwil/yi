# Subagent look in the TUI and console: design proposals

Status: proposal for owner sign-off under the TUI visual approach gate (.ruler/085-tui.md).
Nothing here is built. Owner brief: subagents "look really ugly now".

Mocks are monospace sketches at exactly 80 and 120 columns, each row checked to fit. Colours are named in prose
because a text mock cannot carry them. Durations, names and token counts are illustrative.
Line counts are my estimates from reading the files, not measurements. I read the code and
did not run the binary or capture frames, so what follows describes the code, not a screenshot.

## What exists today

### The card: `TaskCell` and `TaskCell::lines_at` (crates/tui/src/cell.rs)

`TaskCell` holds: `child_id`, `description`, `status` (`TaskStatus`: Running, Done, Failed),
`last_tool`, `prev_tool`, `toolcalls`, `tokens`, `elapsed_ms`, `error`, `spawn` (the
spawning cell's code preview), `answer` (kept to the last 16 KiB, `ANSWER_CAP`) with
`answer_dropped`, `activity` (`ChildActivity`: waiting, writing, executing) and `flag`
(`ChildFlag::NeedsYou` or `Stuck`, each with a note).

`lines_at` builds one title string and a body, then hands both to `boxed`:

- Title: `{glyph} {name}{ · hash} · {state} {elapsed} · {N tool calls}`. The description is
  split by `humanize` into a name and an 8-hex-digit hash tail, so the hash is on screen.
- Glyph and tone: running is the four-frame pulse `◇ ◈ ◆ ◈` (`PULSE_FRAMES`) in purple. Done
  is `↳` in purple. Failed is `✕` in the error colour. A running child with a flag swaps to
  `?` ("needs you") or `!` ("stuck") in the warning colour. The state word is the activity
  (`waiting`, `writing`, `executing`), `done`, `failed`, `needs you` or `stuck`.
- Body, running: a dim row `⚙ {prev_tool}` (an empty row when there is none), a muted row
  `⚙ {tool} · {tokens} tokens`, then the last 4 answer rows, each faded by age (newest
  brightest). More than 4 answer rows is cut silently at the top while running.
- Body, flagged: the note in warning colour, plus the hint "focus it (alt-↓) and type your
  answer" when the note starts with `asks `.
- Body, failed: the error cut to 80 chars in the error colour.
- Body, done: the first 3 answer rows (all rows in Verbose mode), then `… N more lines`,
  plus a `[…] kept the last X of Y B · card cap 16 KiB · alt-↓ opens the child` row when
  the 16 KiB cap trimmed the answer.
- `boxed`: a blank row, `  ╭─ title ───╮`, rows prefixed `  │ ` and suffixed ` │`, then
  `  ╰───╯`. Below 14 columns of width (inner < 8) it drops the box and prints the title
  with a blank above and below. The whole card is one colour (the `tone`), so the border,
  the title and the glyph all share purple, warning or error.

Every card therefore costs at least 4 rows plus a blank (running: top, 2 tool rows, up to 4
answer rows, bottom: 8 rows plus blank). Eight running children cost about 72 rows.

### The scrollback rules (crates/tui/src/app/children.rs)

`ChildRow` carries a `start: Cell`, `ends: Vec<Cell>` and the flags `start_committed`,
`end_committed`, `deferred`, `born_under`. Both are `Cell::Rule { text, accent_name }`:

- Start rule text: `subagent {name} ({id}) · {role} · {task}` (role and task when known).
- End rule text (`end_rule`): `subagent {name} ({id}) · {outcome} · {elapsed} · {N} tokens ·
  {cost} · start {id} · {first line of answer or error}`.
- A `Rule` renders as `── {text} ` followed by `─` fill of `width - label` capped at 40, in
  an accent derived from the child id by `name_accent`. The arm does no truncation, so a long
  end rule is as long as its text. Whether a caller wraps it I did not check.

So a finished child leaves three things in scrollback: a start rule, a card, an end rule,
all repeating the name, and the id appears in the card hash, the start rule, the end rule
and again as `start {id}` in the end rule.

### The roster: `AgentsPopup` (crates/tui/src/agents.rs)

A bottom popup. Header `  agent    tokens   calls   context`, rows hung on ` ├─ ` and ` └─ `,
a `⊙ {spawn}` row once above each family, name in a hash-derived accent (bold when selected),
a 10-cell context bar (`BAR_CELLS`), `tokens_label` as `12k` or `1.2M`, and `x again to stop`
after a first `x`. Glyphs come from `StatusMark`: `◐` working, `○` done, `✕` failed. The code
comment says a needs-you or stuck child reads `◐` here, where the rail says `?` or `!`.
`FINISHED_CARDS = 32` bounds how many finished cards are kept once committed.

### The shared vocabulary (crates/types/src/status_mark.rs)

`StatusMark`: `?` NeedsYou, `◐` Working, `!` Stuck, `●` DoneUnseen, `○` Idle, `✕` Failed,
`·` Unknown, one cell each. The card does not use it for running (pulse) or done (`↳`), so
one child is `◆`, `↳`, `◐`, `○` on four surfaces.

### Console sidebar and avatars (crates/console/src/sidebar.rs, avatar.rs, orb)

`child_rows` hangs at most `CHILD_ROWS = 3` children under their session on `  ├` / `  └`.
Each row is: connector, a two-letter tile (`name_tile`, first two alphanumerics uppercased)
styled in the session's accent, a space, the child's name (full sidebar only), a space, the
`StatusMark` glyph in the status colour. A 1-row kitty-graphics avatar is placed at column 3,
`rows * 2` cells wide, from `avatar_at` with `Art::Identicon` (a hashed grid in the accent),
so the tile text and the image share the same cells. Past 3 the last row reads `  └ +N` on
the rail or `  └ +N more`, with no states in it. The orb (`OrbState`) has a
`Delegating { children: u8 }` state labelled "Delegating…", carrying a count and nothing
about which children are doing what.

### What reads as ugly (my reading of the above)

1. Weight: 8 or more rows per running child, boxed, with a blank above each.
2. Redundancy: name or id four times per child, plus a hex hash in the title.
3. One colour per card: purple border, title and glyph all alike, so nothing leads the eye.
4. Four glyph vocabularies for one thing (`◆ ↳` card, `◐ ○` roster, tiles in the sidebar).
5. No aggregate: 30 children are 30 boxes. The sidebar says `+N more` with no state.
6. The title is one long line with 5 separators that truncates the tail first
   (`boxed` cuts at width - 8), which is where state and elapsed time live.


Current card at 80 columns, running, done, failed:

```text

  ╭─ ◆ review-auth · 3f9a1c2b · writing 42s · 7 tool calls ────────────────────╮
  │ ⚙ read_file                                                                │
  │ ⚙ grep · 12k tokens                                                        │
  │ The refresh path reads the token before the lock is taken,                 │
  │ so two requests can race on the same file. The second                      │
  │ write wins and the first caller keeps a stale token.                       │
  ╰────────────────────────────────────────────────────────────────────────────╯

  ╭─ ↳ review-auth · 3f9a1c2b · done 1m12s · 9 tool calls ─────────────────────╮
  │ The refresh path reads the token before the lock is taken,                 │
  │ so two requests can race on the same file. The second                      │
  │ write wins and the first caller keeps a stale token.                       │
  │ … 11 more lines                                                            │
  ╰────────────────────────────────────────────────────────────────────────────╯

  ╭─ ✕ fix-lockfile · 8e11d0a4 · failed 9s · 2 tool calls ─────────────────────╮
  │ cargo update: failed to fetch index: network unreachable                   │
  ╰────────────────────────────────────────────────────────────────────────────╯
── subagent review-auth (3f9a1c2b) · reviewer · check the refresh path ─────────
```

## Option 1: Ledger

Drop the boxes. Every child is one row in a compact table: glyph, name, state word, elapsed, calls, tokens, then the one thing worth saying (the live tool, the question, the error or the first answer line). Columns align across children, so the eye reads down a column instead of parsing five separators per box. A running child that has more to say (a question, a long answer) does not grow its row. The detail is behind `alt-↓`, which already opens the child.

### Mocks

**80 columns**

Running (2 working, 1 needs you):

```text
  ◐ review-auth   writing    42s  7 calls   12k  ⚙ grep
  ◐ migrate-db    executing 1m05 12 calls   31k  ⚙ cargo test -p yi-db
  ? fix-lockfile  needs you   9s  2 calls    3k  asks: keep the old lock? alt-↓…
```

Done:

```text
  ○ review-auth   done      1m12  9 calls   14k  The refresh path reads the tok…
```

Failed:

```text
  ✕ fix-lockfile  failed      9s  2 calls    3k  cargo update: failed to fetch …
```

Many children (30 spawned):

```text
  subagents 30   ◐ 12   ? 1   ! 1   ○ 15   ✕ 1
  ? fix-lockfile  needs you   9s  2 calls    3k  asks: keep the old lock? alt-↓…
  ! probe-ci      stuck     4m10 31 calls   88k  no output for 3m, last ⚙ gh ru…
  ✕ scan-deps     failed    2m02 14 calls   40k  cargo update: failed to fetch …
  ◐ review-auth   executing  20s  3 calls    4k  ⚙ grep
  ◐ migrate-db    writing    31s  4 calls    7k  ⚙ read_file
  ◐ lint-docs     executing  42s  5 calls   10k  ⚙ cargo check
  … +24 more: ◐ 9 running  ○ 15 done   alt-↓ roster
```

**120 columns**

Running (2 working, 1 needs you):

```text
  ◐ review-auth   writing    42s  7 calls   12k  ⚙ grep
  ◐ migrate-db    executing 1m05 12 calls   31k  ⚙ cargo test -p yi-db
  ? fix-lockfile  needs you   9s  2 calls    3k  asks: keep the old lock? alt-↓ to answer
```

Done:

```text
  ○ review-auth   done      1m12  9 calls   14k  The refresh path reads the token before the lock is taken, so two requ…
```

Failed:

```text
  ✕ fix-lockfile  failed      9s  2 calls    3k  cargo update: failed to fetch index: network unreachable
```

Many children (30 spawned):

```text
  subagents 30   ◐ 12   ? 1   ! 1   ○ 15   ✕ 1
  ? fix-lockfile  needs you   9s  2 calls    3k  asks: keep the old lock? alt-↓ to answer
  ! probe-ci      stuck     4m10 31 calls   88k  no output for 3m, last ⚙ gh run watch
  ✕ scan-deps     failed    2m02 14 calls   40k  cargo update: failed to fetch index
  ◐ review-auth   executing  20s  3 calls    4k  ⚙ grep
  ◐ migrate-db    writing    31s  4 calls    7k  ⚙ read_file
  ◐ lint-docs     executing  42s  5 calls   10k  ⚙ cargo check
  … +24 more: ◐ 9 running  ○ 15 done   alt-↓ roster
```

### State vocabulary and glyphs

One vocabulary on every surface: the existing `StatusMark` table in crates/types/src/status_mark.rs, unchanged. Working `◐`, needs you `?`, stuck `!`, done `○` (`●` while unseen), failed `✕`. The card's `◇ ◈ ◆ ◈` pulse and `↳` go away, so a child is the same glyph in the card, roster, sidebar and rail. The activity word (`waiting`, `writing`, `executing`) stays as text beside the glyph.

Row grammar: `{glyph} {name} {state} {elapsed} {N} calls {tokens}  {tail}`. Tail by state: running shows `⚙ {tool}`, needs you shows the child's note, stuck shows the note, done shows the first answer line, failed shows the first error line. The 8-hex hash is not shown (it stays in `alt-↓` and the roster).

### Colour and accent

Name is drawn in the child's hash accent (`name_accent`, the same colour the roster already uses). The glyph alone carries status colour: working in the purple the pulse uses today, `?` in the existing blocked colour, `!` in warning, `○` in the idle colour, `✕` in error. The tail is muted, except a needs-you note, which is warning and the only row that is not muted. Today the whole card is one colour. Here the one coloured thing per row is the one that changed.

### Scaling from 1 to 30 children

1 child: one row. 3 children: three rows, aligned. 8 children: 8 rows (today about 72). 30 children: a header with counts, the children that need you or are stuck first (pinned), then the 4 to 6 most recently active, then a `… +N more` row that names the counts of the rest (`◐ 9 running  ○ 14 done  ✕ 1 failed`) and the call that opens the roster. The cap is a named constant (for example `LEDGER_ROWS = 8`) and the cut row follows the loud-caps rule: kept of total, the cap, the next call. A failure is never hidden by the cap: needs-you, stuck and failed rank above running.

### Scrollback

Rows stay in the live region while the child runs. When a child ends, its row is committed once, as a single line in its final state (done or failed), at the point in the transcript where it finished. No start rule and no end rule: the committed row carries name, outcome, elapsed, tokens, cost and the first answer line, which is what the end rule carries today minus the repeated id. The 16 KiB answer cap and its `[…]` notice move to the detail view, where the full answer is read.

### Code changes

- `crates/tui/src/cell.rs`: replace the `boxed` use in `TaskCell::lines_at` with a one-row renderer and a small column-width helper (name width = longest live name, capped). About -90 / +110 lines. `humanize` stops splitting the hash.
- `crates/tui/src/app/children.rs`: drop the start `Cell::Rule` and fold `end_rule` into the committed row. About -45 / +25. `ChildRow` loses `start`, `start_committed` and most of `deferred`/`born_under` if ordering no longer needs a start anchor (needs a read of the ordering code, which I did not do).
- Live-region ordering and the row cap (rank, pin, `+N` row): about +70.
- `crates/tui/src/agents.rs`, `crates/console/src/sidebar.rs`: no change needed for correctness. Optional: the sidebar's `+N more` row gains the state counts, about +15.
- Tests: card rendering tests in crates/tui/tests/cards.rs are rewritten (7 hits for `TaskCell` there), plus one cap test that trips `LEDGER_ROWS` and reads the notice.

Estimate: net about +60 to +120 lines of src, test churn larger than src. This is my estimate from reading, not a measurement.

### Risks

- The answer preview shrinks from three rows to one line. A reader who liked seeing three lines of a finished answer loses them in scrollback. Mitigation: `Verbose` mode can keep the three-row form.
- Fixed columns waste space at 120 columns and squeeze the tail at 80. The mock at 80 already cuts the done tail at the width, with `…`.
- Aligned columns need the longest live name, so one long name moves everyone's columns. A cap on name width fixes it.
- An in-flight needs-you question may need more than one row. It is the one case where the ledger can break its one-row rule, and I would rather show the question in the tail and open the child than grow the row.


## Option 2: Gutter card

Keep a card, lose the box. Each child is a short stack of rows hung on a coloured left gutter bar `▎` in the child's own accent, like a quoted block. The first row is a title (name, glyph, state, elapsed, calls, tokens), the rows under it are the body. There is no top, bottom or right border, no blank row between cards, and no width-dependent truncation of the right edge, so nothing is lost when the terminal is narrow. This is the smallest change from today and keeps the answer text visible.

### Mocks

**80 columns**

Running (2 working, 1 needs you):

```text
  ▎ review-auth ◐ writing 42s · 7 calls · 12k
  ▎ ⚙ grep · …the lock is taken, so two requests can race on the same file
  ▎ migrate-db ◐ executing 1m05 · 12 calls · 31k
  ▎ ⚙ cargo test -p yi-db · running 14 tests
  ▎ fix-lockfile ? needs you 9s · 2 calls
  ▎ asks: keep the old lock?
  ▎ focus it (alt-↓) and type your answer
```

Done:

```text
  ▎ review-auth ○ done 1m12 · 9 calls · 14k · $0.02
  ▎ The refresh path reads the token before the lock is taken, so two requests
  ▎  can race on the same file and the first caller keeps a stale token.
  ▎ … +9 lines · alt-↓ opens
```

Failed:

```text
  ▎ fix-lockfile ✕ failed 9s · 2 calls
  ▎ cargo update: failed to fetch index: network unreachable
```

Many children (30 spawned):

```text
  ▎ fix-lockfile ? needs you 9s
  ▎ asks: keep the old lock?
  ▎ probe-ci ! stuck 4m10
  ▎ no output for 3m, last ⚙ gh run watch
  ▎ scan-deps ✕ failed 2m02
  ▎ cargo update: failed to fetch index
  ▎ review-auth ◐ writing 42s · ⚙ grep
  ▎ migrate-db ◐ executing 1m05 · ⚙ cargo test
  ▎ lint-docs     ◐
  ▎ port-tests    ◐
  ▎ bench-io      ◐
  ▎ +22 more: ◐ 7  ○ 15   alt-↓ roster
```

**120 columns**

Running (2 working, 1 needs you):

```text
  ▎ review-auth ◐ writing 42s · 7 calls · 12k
  ▎ ⚙ grep · …the lock is taken, so two requests can race on the same file
  ▎ migrate-db ◐ executing 1m05 · 12 calls · 31k
  ▎ ⚙ cargo test -p yi-db · running 14 tests
  ▎ fix-lockfile ? needs you 9s · 2 calls
  ▎ asks: keep the old lock?
  ▎ focus it (alt-↓) and type your answer
```

Done:

```text
  ▎ review-auth ○ done 1m12 · 9 calls · 14k · $0.02
  ▎ The refresh path reads the token before the lock is taken, so two requests can race on the same file and the first
  ▎  caller keeps a stale token.
  ▎ … +9 lines · alt-↓ opens
```

Failed:

```text
  ▎ fix-lockfile ✕ failed 9s · 2 calls
  ▎ cargo update: failed to fetch index: network unreachable
```

Many children (30 spawned):

```text
  ▎ fix-lockfile ? needs you 9s
  ▎ asks: keep the old lock?
  ▎ probe-ci ! stuck 4m10
  ▎ no output for 3m, last ⚙ gh run watch
  ▎ scan-deps ✕ failed 2m02
  ▎ cargo update: failed to fetch index
  ▎ review-auth ◐ writing 42s · ⚙ grep
  ▎ migrate-db ◐ executing 1m05 · ⚙ cargo test
  ▎ lint-docs     ◐
  ▎ port-tests    ◐
  ▎ bench-io      ◐
  ▎ +22 more: ◐ 7  ○ 15   alt-↓ roster
```

### State vocabulary and glyphs

One vocabulary on every surface: the existing `StatusMark` table in crates/types/src/status_mark.rs, unchanged. Working `◐`, needs you `?`, stuck `!`, done `○` (`●` while unseen), failed `✕`. The card's `◇ ◈ ◆ ◈` pulse and `↳` go away, so a child is the same glyph in the card, roster, sidebar and rail. The activity word (`waiting`, `writing`, `executing`) stays as text beside the glyph.

Title grammar: `▎ {name} {glyph} {state} {elapsed} · {N} calls · {tokens}`. Body rows by state: running, the live tool plus the last answer row; needs you, the note and the hint; failed, the error; done, up to 2 answer rows plus `… +N lines · alt-↓ opens`.

### Colour and accent

The gutter bar `▎` is the child's hash accent, so a child is recognisable at a glance in a column of cards, which today's single purple box does not allow. The name is bold in the same accent. The glyph takes the status colour. Body text is muted; the answer text under a done child is normal weight. A needs-you or stuck card draws the gutter in the warning colour for the whole card, so it cannot be missed in a long scrollback, and goes back to the accent when answered.

### Scaling from 1 to 30 children

1 child: 2 to 4 rows. 8 children: about 24 to 32 rows. 30 children: too tall, so past a threshold (for example 6 live cards) older running and all done cards fold to title-only rows, then past 12 to a count row (`+22 more: ◐ 7  ○ 15`). Folding is by rank: needs-you, stuck and failed keep their full cards. The threshold and fold are named constants with the `[…]` row naming kept of total and the call that opens the roster. This is the weakest scaling of the three options, and the mock shows the fold.

### Scrollback

Whole cards commit to scrollback when the child finishes, as today, but the start and end rules are removed because the gutter title already names the child and the outcome. The committed card is the final-state card (done: title plus up to 2 answer rows). The 16 KiB cap notice stays, as one row under the body. Scrollback reads as a run of coloured quote blocks, one per child.

### Code changes

- `crates/tui/src/cell.rs`: replace `boxed` for `TaskCell` with a gutter renderer (prefix every row with `  ▎ `, no right border, no padding). About -40 / +50. `TaskCell::lines_at` body logic stays.
- `crates/tui/src/app/children.rs`: delete the start rule and end rule construction, or keep them behind Verbose. About -60 / +10.
- Fold logic for many children: about +50.
- `crates/tui/src/colors.rs`: none, `name_accent` already exists.
- Tests: cards.rs updates, one fold test.

Estimate: net about -30 to +30 lines of src, the smallest of the three.

### Risks

- Still the heaviest per child: a running card is 3 to 7 rows, so 30 children is mostly folded and the fold does the work, not the design.
- A one-colour left bar reads well on dark themes and is faint on light ones. Needs a look on both before sign-off.
- Removing the rules changes the scrollback's visual landmarks for people who search for `subagent ... (id)`. The id would have to stay somewhere, for instance in the committed title as a dim tail.
- The least change means the least relief from the owner's complaint: it fixes the box, the blank rows and the colour, but not the weight.


## Option 3: Strip and grid

Treat subagents as one live object, not N cards. A single strip pinned in the live region shows a count header (`◐ 2  ? 1  ○ 14  ✕ 1`, tokens, time) and then one line per child that needs attention. Everything else is a dense grid of two-letter tiles with the status glyph (`RE◐ MI◐ FI?`), the same `name_tile` the console sidebar already draws. The tile is the child's identity in every surface: strip, sidebar, rail and orb. Detail for any child is one `alt-↓` away.

### Mocks

**80 columns**

Running (2 working, 1 needs you):

```text
  subagents  ◐ 2  ? 1   32k tok · 1m05
  RE◐ review-auth   writing    ⚙ grep
  MI◐ migrate-db    executing  ⚙ cargo test -p yi-db
  FI? fix-lockfile  asks: keep the old lock? alt-↓ to answer
```

Done:

```text
  subagents  ○ 1   14k tok · $0.02 · 1m12
  RE○ review-auth   done 1m12 · 9 calls · alt-↓ opens
```

Failed:

```text
  subagents  ✕ 1   3k tok · 9s
  FI✕ fix-lockfile  cargo update: failed to fetch index: network unreachable
```

Many children (30 spawned):

```text
  subagents  ◐ 12  ? 1  ! 1  ○ 15  ✕ 1   411k tok · $3.10 · 4m10
  RE◐ MI◐ SC◐ LI◐ PO◐ BE◐ GE◐ SY◐ RB◐ MB◐ TB◐ DB◐ FI? PR! ZB✕
  PB○ BB○ DD○ GB○ RC○ MC○ TC○ AC○ CC○ DC○ EC○ HC○ IC○ JC○ KC○
  ? FI fix-lockfile  asks: keep the old lock? alt-↓ to answer
  ! PR probe-ci      stuck 4m10: no output for 3m
  ✕ ZB scan-deps     cargo update: failed to fetch index
```

**120 columns**

Running (2 working, 1 needs you):

```text
  subagents  ◐ 2  ? 1   32k tok · 1m05
  RE◐ review-auth   writing    ⚙ grep
  MI◐ migrate-db    executing  ⚙ cargo test -p yi-db
  FI? fix-lockfile  asks: keep the old lock? alt-↓ to answer
```

Done:

```text
  subagents  ○ 1   14k tok · $0.02 · 1m12
  RE○ review-auth   done 1m12 · 9 calls · alt-↓ opens
```

Failed:

```text
  subagents  ✕ 1   3k tok · 9s
  FI✕ fix-lockfile  cargo update: failed to fetch index: network unreachable
```

Many children (30 spawned):

```text
  subagents  ◐ 12  ? 1  ! 1  ○ 15  ✕ 1   411k tok · $3.10 · 4m10
  RE◐ MI◐ SC◐ LI◐ PO◐ BE◐ GE◐ SY◐ RB◐ MB◐ TB◐ DB◐ FI? PR! ZB✕
  PB○ BB○ DD○ GB○ RC○ MC○ TC○ AC○ CC○ DC○ EC○ HC○ IC○ JC○ KC○
  ? FI fix-lockfile  asks: keep the old lock? alt-↓ to answer
  ! PR probe-ci      stuck 4m10: no output for 3m
  ✕ ZB scan-deps     cargo update: failed to fetch index
```

### State vocabulary and glyphs

One vocabulary on every surface: the existing `StatusMark` table in crates/types/src/status_mark.rs, unchanged. Working `◐`, needs you `?`, stuck `!`, done `○` (`●` while unseen), failed `✕`. The card's `◇ ◈ ◆ ◈` pulse and `↳` go away, so a child is the same glyph in the card, roster, sidebar and rail. The activity word (`waiting`, `writing`, `executing`) stays as text beside the glyph.

Header: counts per `StatusMark`, then total tokens, cost and the longest elapsed. Tile: `{two letters}{glyph}`. Attention row: `{glyph} {name} {note or error}`, shown for needs-you, stuck and failed only, so the user sees what needs them and nothing else.

### Colour and accent

A tile letter pair is in the child's hash accent and the glyph next to it takes the status colour, which is exactly how the console sidebar's `tile_span` plus `mark_style` already read, so the TUI and console would finally match. The header counts are coloured by their glyph. Attention rows are warning or error coloured for their whole text. A quiet strip is almost all muted and accent, so the one warning row is the loudest thing on screen.

### Scaling from 1 to 30 children

1 child: header plus one tile, or one ledger-style row (below a threshold of 3, show rows instead of tiles). 8 children: header plus one tile row. 30 children: header plus 2 tile rows at 80 columns (about 19 tiles a row at 4 cells each) or 1 to 2 at 120. The height is bounded by the attention rows plus ceil(N / tiles per row), so growth is sublinear. Attention rows cap at a named constant with a `[…]` row. Tile collisions (two children named `review-auth` and `report-card`) are real: disambiguate with a third character or the existing `accent_index` colour, and the mock keeps pairs distinct by hand, which the real packer would have to do.

### Scrollback

The strip is live-region only and is never committed as a whole. On finish, a child commits at most one row: failed children and children that produced an answer get a single ledger-style line in scrollback; a child that finished quietly commits nothing and appears only in the closing summary. When the last child ends, the strip commits one summary row (`subagents 30 · ○ 29 ✕ 1 · 411k tok · $3.10 · 4m10`). Scrollback is the shortest of the three options, which is also its cost: you cannot scroll back to read a finished child's streamed answer, you open it.

### Code changes

- New `crates/tui/src/cell.rs` (or a new module under crates/tui/src) renderer for a strip cell fed by the set of live children, plus the tile packer. About +180.
- `crates/tui/src/app/children.rs`: children no longer each push a `TaskCell`, they update one strip cell; the start and end `Rule` become one committed row at finish. This is the largest change here, about -80 / +120 and it touches the commit ordering (`deferred`, `born_under`) that I did not trace.
- `crates/console/src/sidebar.rs`: raise `CHILD_ROWS = 3` or replace it by a state-count suffix on the session row; about +25.
- `crates/orb`: `Delegating { children }` could carry counts by state; optional, about +20 across `OrbState` and its label.
- Tests: strip packing at 80 and 120, a 30-child case with one failure, the cap notice.

Estimate: net about +250 to +350 lines of src. The largest change, and it trips the +150 free growth band, so it needs a `growth:` memo naming the deletions weighed.

### Risks

- Biggest change and the biggest behaviour change: streamed child answers are no longer visible in the main transcript. That is a product decision, not a style one.
- Two-letter tiles collide and are meaningless for names such as `agent-1`, `agent-2`. A numeric suffix or a different tile source is needed.
- A pinned strip uses live-region rows all the time, even when one child runs.
- It is the only option with a real answer for 30 children, and the only one that changes what scrollback contains.

## Recommendation

Pick Option 1, the ledger, and take two things from the others.

Why: the owner's complaint is that a subagent looks ugly. Reading the code, the ugliness is weight and noise (a boxed card of 5 to 9 rows per child, one colour, the name or id up to four times, a hex hash in the title, four glyph sets). Option 1 removes all of that for the least behaviour change: a child is still a thing in the transcript, still committed once when it ends, still opened with `alt-↓`. It scales to 30 children through a ranked cap that cannot hide a failure. It is also the option that makes the TUI card, the roster and the sidebar speak the same `StatusMark` vocabulary without touching the roster or the sidebar.

Take from Option 3: the state-count header and the sidebar suffix (`◐ 12  ○ 15  ✕ 1` in place of `+N more`), since the aggregate is what 30 children really need, and it is cheap (about +40 lines). Take from Option 2: the gutter in warning colour for a needs-you child when its question must be shown in more than one row.

Why not Option 2: smallest diff, but it keeps the weight, and I think the owner would still call it heavy. Why not Option 3 first: it changes what scrollback holds, which is a product decision and the largest diff (+250 to +350 lines, over the free growth band). It is the right second step if 30-child runs turn out to be common.

Assumptions I made, none checked with the owner: that the owner wants less height more than more detail; that finished answers need not be visible in scrollback beyond one line; that `StatusMark` is the vocabulary to keep.

## Sign-off questions

1. Is one answer line in scrollback enough for a finished child, or should three rows stay?
2. Ledger columns (Option 1) or title-and-body rows with a gutter (Option 2)?
3. Should the console sidebar and the orb carry state counts now, or follow later?

Under the visual approach gate, no code is written until this is signed off. After sign-off, the proof is `just tui-see run` before and after on a scene with 1, 3 and 30 children, plus the cell diff, per .ruler/085-tui.md. A 30-child scene does not exist yet in scripts/proof/tui_see (not checked), so it would be added first.
