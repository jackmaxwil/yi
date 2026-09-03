# Tool ergonomics, telemetry, and TUI streaming stability

```
status:  LANDED 0.67.0 (2026-08-29), all eleven phases — see ARCHITECTURE.md's
         0.67.0 row for what shipped and the one recorded deviation
         (LaneRecord::ToolStarted stays unwritten; timing lives in details).
         P0.1's ToolStarted step and P10.4's delta-clone measurement moved to
         the parking lot. Cross-repo grid rows (§14) remain open in the grid
         repo. File:line references below describe the pre-change tree at
         8256d26 and are historical.
date:    2026-08-29
inputs:  a live Yi session's tool self-assessment (bash/read/edit/grep/
         ipython/grid ratings) · three-scout study (2026-08-29): the reference
         <ref> (tools/, apply-patch/, unified_exec/, otel/) · another reference
         packages/coding-agent + packages/hashline · pi/the reference/the reference/the reference/
         deepseek-harness tool trees · Yi ground truth: crates/tools/src
         (builtins.rs, hashline/, reduce.rs, process.rs, ignore.rs),
         crates/loop/src/run.rs, crates/runtime/src/kernel.rs,
         crates/session/src/state.rs, crates/types/src/record.rs,
         crates/tui/src (highlight.rs, orb/, markdown.rs, app.rs, render.rs)
         · docs/TODOS.md rows C2, C8, M1, M2, M3 · docs/size-ledger.md
ruling:  the `regex` crate ban is lifted (maintainer decision, 2026-08-29).
         The unban is recorded with its first consumer (P3), never as a
         standalone doc edit: deny.toml entry out, YI_DESIGN §13.6 ban list
         amended, size-ledger row for the measured delta.
```

## 1. Thesis

The toolset optimizes safety-per-token and it shows: content-hash edit
anchors, loud failures, reduce with recovery tees. What it lacks is the
cheap half of ergonomics — caps that name the exact next call, search that
can anchor an edit, reads that follow the shape of code instead of its line
order — and any instrumentation at all, so every tuning argument is an
anecdote. Three TUI defects ride along because they are the same discipline
applied to the render path: the highlighter has never been audited against a
corpus, the kitty orb deletes itself before repainting, and streamed
markdown reaches scrollback only at blank lines.

Eleven phases, dependency-ordered, independently landable; each is its own
version bump + changelog row. P0 lands first because every later "did it
help" question reads its numbers.

| phase | what | size |
|---|---|---|
| P0 | telemetry spine: ToolStarted, durations, sizes, error kinds, `yi stats` | M |
| P1 | read hardening: byte cap, line clip, honest notices, bigger explicit reads | S |
| P2 | multi-range read (grid `scope` becomes the read-planner) | S |
| P3 | grep v2: regex (unban lands here), include/type filter, pagination, tag minting | M |
| P4 | parallel execution for read-kind calls (the discarded `sequential` flag) | S |
| P5 | ipython prewarm | S |
| P6 | output-budget unification: universal spill-to-file, per-call bash budget | S |
| P7 | edit follow-through: C8 freeform grammar, stale-window check, M3 stats | M |
| P8 | highlight audit + fix | S–M |
| P9 | kitty orb stability | S |
| P10 | markdown streaming newline design | M |

P1→P2 ordered (ranges build on hardened caps). P3 reuses P1's notice
helpers. P7's M3 half needs P0. P8–P10 are order-free and can interleave.

## 2. Ground truth (verified 2026-08-29, worktree at 8256d26)

Tools:

- `read` caps lines only: `READ_LINE_CAP = 2_000` (hashline/tool.rs:17); no
  byte cap, no per-line clip — `format_numbered_line` emits lines verbatim
  (hashline/format.rs:111). One minified line floods the context. The
  trailing notice misstates: "of {line_count} more lines available" prints
  the total, not the remainder (hashline/tool.rs:129).
- `grep` is a literal substring scan that allocates a `String` per file and
  lowercases every line under `ignore_case` (builtins.rs:246-266); caps at
  200 hits with a bare `[result capped]` marker (builtins.rs:12,310); mints
  no snapshot tags, so its hits cannot anchor edits. hashline/prompt.md:4
  promises tags from "latest `read`/`search`" — no search tool exists.
  Doc/tool drift.
- `edit` already re-grounds: every changed hunk gets a fresh
  `[path#NEWTAG]` header plus a ±3-line renumbered window, recorded as seen
  (hashline/tool.rs:158-175). Mismatch errors show the live hash and
  anchored context (hashline/mismatch.rs:57-92). The ceremony's remaining
  taxes are the JSON-escaping of `+` body rows (TODOS C8) and the absence of
  any measured failure-rate data (TODOS M3).
- Tool batches execute serially: run.rs computes `sequential` then discards
  it — `let _ = sequential;` (crates/loop/src/run.rs:202-229).
  `ExecutionMode::Parallel` (crates/loop/src/tool.rs:21) is dead code.
- The kernel "boots on first cell" (crates/runtime/src/kernel.rs:188); the
  first ipython call pays venv probe + ipykernel spawn + zmq handshake +
  rlm bootstrap.
- Observability is zero by design: no tracing/log/metrics dependency
  anywhere (0.59.0 dropped the tracing tree). `LaneRecord::ToolStarted`
  exists in the schema, is conformance-tested, and is never constructed
  (crates/types/src/record.rs:82-95). Only ipython measures duration.
  `SessionStats` accumulates tokens only from `LaneRecord::Usage`, which
  only child lanes write — main-lane tokens report ~zero
  (crates/session/src/state.rs:201-211, record.rs:231-234). Bash is the
  only tool reporting `rawBytes`/`outBytes`, and nothing reads them back.

TUI:

- highlight.rs is a per-line scanner, five languages, no cross-line state,
  4 KiB line cap. Rust's quote set includes `'` (highlight.rs:31), so a
  lifetime (`'a`) likely opens an unterminated "string" — never verified
  against a corpus; there are no fixture tests.
- The orb repaints by deleting every placement of its image id and
  re-transmitting the full zlib-deflated RGBA frame (orb/kitty.rs:138-156),
  and re-emits whenever the placement cell moves
  (orb/mod.rs:152 `state.at != Some((col, row))`) — so a fast-scrolling
  transcript forces delete + full retransmit per frame, outside any
  synchronized-output bracket.
- Streamed markdown commits to scrollback only at a blank line outside a
  fence (`stable_cut`, markdown.rs:25-42; app.rs:460-484). A long fence or
  list commits nothing until it ends. `commit_complete_source` — the reference
  newline gate — is dead code, used only by a unit test (markdown.rs:15,
  tui_unit.rs:127). Every SSE delta clones the entire partial message
  (run.rs:325-345) and the live tail re-renders from scratch each frame
  (render.rs:112-123): O(n²) over a long turn.
- ignore.rs always skips `.git` and reads per-directory `.gitignore`; it
  does not read `.git/info/exclude` or the user's global excludes (noted,
  not scheduled).

Donor facts this plan leans on (scout study, 2026-08-29):

- the reference has no grep/read tool at all — shell + `rg` by prompt, said
  in its system prompt; its old search tool is `Stage::Removed`. Its
  transferable pieces: the apply_patch 4-pass anchor ladder
  (`apply-patch/src/seek_sequence.rs:12-114`), two-layer truncation — 1 MiB
  head/tail capture then middle-out token budget, model-settable per call
  (`unified_exec/mod.rs:74-77`, `utils/string/truncate.rs`) — and per-tool
  telemetry: `duration_ms`, `output_truncated`, counter + histogram, and a
  `command_category` tag derived from parsing shell argv
  (`core/src/tools/registry.rs:624-638`).
- the reference is the hashline donor, and its *read* half was never ported:
  path-embedded selectors with multi-range (`:1-5,20-30`,
  `read-selector.ts:33-68`), outline-by-default with a content-hash LRU
  (`read-summary.ts:1-40`), byte budget scaling with requested lines
  (`read.ts:1560`), and grep recording seen lines (`grep.ts:1519`) so
  search output can anchor edits.
- pi: every cap notice names the exact next call, including the
  single-long-line escape hatch `[Line 42 is 3.1MB … Use bash: sed -n '42p'
  …]` (`core/tools/read.ts:297-301`, `grep.ts:344-361`).
- the reference: grep paginates by `offset` and distinguishes *which* cap was hit —
  page cap vs collection cap vs unscanned files
  (`grep_files.zig:344`, `grep_search.zig:14-17`).
- the reference: near-miss edits are diagnosed, never silently fuzzy-applied
  ("found after trimming whitespace — retry", `edit.rs:263-274`); reads log
  a structured warning whenever they return truncated output
  (`read.rs:241-250`).
- the reference: one universal post-execute truncation wrapper spills full text
  to a dated file and tailors the recovery hint (`tool/truncate.ts:85-141`,
  wired at `tool/tool.ts:131-144`).
- deepseek: rg invoked flag-value style (`--regexp=<pattern>`) so a
  leading-dash pattern can never parse as a flag (`grep.ts:113-115`);
  read-before-edit enforced by a separate policy layer, not the tool.

## 3. P0 — telemetry spine

The repo's own culture (gates, ledgers, ratchets) applied to tool calls.
No tracing dependency; the session JSONL is the ledger and `yi stats`
replays it.

1. Write `LaneRecord::ToolStarted` at dispatch (crates/loop/src/run.rs,
   beside the `ToolExecutionStart` emit). The type and its conformance
   fixtures already exist; this is a writer, not a schema change.
2. Wrap the `execute_one` await (run.rs:216) in an `Instant`; add
   `durationMs` to `ToolExecutionEnd` and to the persisted `ToolResult`.
3. Promote size accounting out of bash-only details: `rawBytes`,
   `outBytes`, `truncated` become fields every tool fills (read, grep,
   glob, edit, write, ipython). The reduce path already computes them for
   bash (reduce.rs:32-73); the rest are one subtraction each.
4. Error taxonomy: an `errorKind` enum on failed results — `denied`,
   `not_found`, `invalid_args`, `aborted`, `stale_tag`, `noop_loop`,
   `tool_error` — replacing today's boolean + prose collapse. The hashline
   mismatch path sets `stale_tag` (this is the M3 prerequisite).
5. Fix `SessionStats`: accumulate main-lane usage from assistant entries
   (state.rs:201-211 currently counts only child-lane `LaneRecord::Usage`).
6. Bash command classification: extend the existing argv screen
   (builtins.rs:345-373) to a `category` detail — `read | list_files |
   search | build | test | vcs | unknown` — the reference's `command_category`,
   which is what makes "the model fell back to shell rg" measurable.
7. `yi stats [session]`: per-tool call count, p50/p95 duration, failure
   rate by kind, truncation counts, bytes in/out, chars/4 token estimates,
   and read-efficiency (seen lines later edited ÷ lines shown — the
   snapshot store's seen-lines ledger already holds the data). Porcelain
   columns + `--json`, exit-0 discipline.

done: a full-suite run followed by `yi stats` answers every question in
this section from the JSONL alone; no new dependency; conformance fixtures
extended for the new writer.

## 4. P1 — read hardening

Caps first, honesty second, size third. All numbers config-overridable,
defaults stated in the tool description (pi does this; the model then
plans around them).

1. Byte cap: 50 KiB default model-facing budget alongside the 2 000-line
   cap, whichever trips first (the reference/pi/deepseek converge on 50 KiB;
   the reference's 256 KiB is the ceiling shape). Explicit `limit` may raise the line
   cap; the byte budget scales with it — the reference's
   `max(50 KiB, lines × 512)` — so a deliberate big read is allowed and a
   runaway one is not.
2. Per-line clip at 2 000 chars with an in-band marker
   `… (line truncated to 2000 chars)`; when a *single* line exceeds the
   whole byte budget, emit pi's escape hatch verbatim:
   `[Line N is X. Use bash: sed -n 'Np' path | head -c 51200]`.
3. Honest notices: fix the "more lines available" wording
   (hashline/tool.rs:129) to state total, shown range, and the exact next
   call (`offset=N`); distinguish "line cap", "byte cap", and "end of
   file" endings (the reference's three-way sentinel).
4. Record `rawBytes`/`outBytes`/`truncated` per P0.3.

done: reading a minified bundle costs ≤ the byte budget; every truncated
read names the next call; snapshot/seen-lines semantics unchanged (clipped
rows are still *seen* — the tag hashes the file, not the rendering).

## 5. P2 — multi-range read

Chunks are not linear: what a model needs around an edit is definition +
callers + types, scattered across a file. the reference ships this as `:1-5,20-30`
selectors; Agentless-style skeleton→span localization is the published
evidence; Yi's seen-lines ledger already makes elision safe (an edit
anchored on an unseen line is already rejected).

1. `ranges: [[start, end], …]` param on `read` (exclusive with
   `offset`/`limit`). One call, one snapshot, one tag, N windows, elision
   markers between windows, every rendered row recorded as seen. Ranges
   are clamped, sorted, merged on overlap; the per-window and total byte
   budgets are P1's.
2. Teach the loop: hashline/prompt.md and the grid skill both learn the
   pattern "grid scope X --depth 2 → read ranges". The grid-side `--ranges`
   output format is a cross-repo row (see §14).

done: `grid scope` output pasted into one `read` call yields an editable
context pack; a hunk across an elision is still rejected; fixture test
covering merge/clamp/seen-lines.

## 6. P3 — grep v2

The unban's first consumer; deny.toml's `regex` entry, the YI_DESIGN §13.6
ban list, and a size-ledger row land in this phase's first commit.

1. Dependency: `regex` with `default-features = false, features = ["std",
   "perf"]`. Everything it pulls (regex-automata, regex-syntax,
   aho-corasick, memchr) is already in the lock via globset — the wrapper
   crate is the only new node. Measure the ladder for the ledger row:
   `std` alone, `+perf`, `+unicode-case`; take unicode only if a real
   pattern needs it (ASCII case-insensitivity covers code search).
2. Semantics: literal by default, `regex: true` opt-in (the reference's default;
   accidental-regex noise dies). Literal path drops the per-file `String`
   + per-line lowercase for `memchr::memmem::Finder` over bytes
   (aho-corasick's ASCII-case-insensitive automaton for `ignore_case`).
3. Filters: `include` glob (globset, already in-tree) and a ~10-entry
   `type` map (rust, py, ts, js, md, toml, json, sh, yaml, html). Not
   ripgrep's 250-type table.
4. Pagination over caps: `offset` param; the footer names *which*
   limit tripped — page cap ("use offset=200"), collection cap ("N files
   were never scanned — narrow the tree"), so "more exists" and "too
   broad" stop being the same message. Add `files_with_matches` mode.
5. Tag minting: every file with hits is snapshotted and its emitted rows
   recorded as seen — grep output becomes a valid edit anchor, closing the
   prompt.md "read/search" drift and deleting one read round-trip from
   every search-driven edit.
6. Keep the description's honesty: bash `rg` remains the documented path
   for multiline and PCRE-shaped work.

done: ledger row recorded; `grep pattern --type rust --offset 200` works;
an edit anchored on a grep hit applies without an intervening read;
literal path benchmarked no slower than today on this repo.

## 7. P4 — parallel read-kind execution

The flag exists and is discarded (run.rs:229). Honor it: within one batch,
calls whose tools are `ToolKind::Read` (and `execution_mode() == Parallel`)
run concurrently under `spawn_blocking` + `join_all`; the first Write/Exec
call and everything after it stays serial, preserving today's ordering
guarantees for mutations. Events still emit per call; results return in
call order regardless of completion order (the transcript stays stable).

Note for P0: concurrent execution breaks the "consecutive timestamps
approximate duration" fallback — which is why P0's explicit `durationMs`
lands first.

done: a 4-read batch's wall clock ≈ its slowest member; mutation batches
byte-identical to today; interrupt (`signal.is_fired`) still aborts
stragglers.

## 8. P5 — ipython prewarm

Fire `KernelService::ensure()` on a background tokio task at session open,
gated on the ipython tool being registered and on config
(`kernel.prewarm`, default on). The first real cell then pays ~0. An idle
timeout is deliberately *not* added — the kernel is the session's
scratchpad; killing it loses state for a few MB of RSS. The one-time venv
bootstrap stays visible (progress lines already exist) and is never run by
prewarm without the same consent path it has today.

done: time-to-first-cell-result on a warm venv drops from spawn+handshake
to execution only; `yi stats` (P0) shows the delta; no kernel process for
sessions that never had the tool registered.

## 9. P6 — output-budget unification

1. Universal spill-to-file: generalize bash's tee (reduce.rs:199) into a
   post-execute hook on every tool — any result over its budget writes the
   full text under the recovery dir and the marker carries the path
   (the reference's wrapper, minus the agent-aware hint until subagents need
   it). read/grep/glob caps stop being silent dead ends.
2. Per-call budget on bash: `max_output_lines` (or tokens) param, min'd
   against the global cap — the reference's `max_output_tokens`. The model raises
   it when it *knows* (`-v` runs, test suites); `RAW_FLAGS` keeps working
   for the cases the model forgets.

done: every `[truncated]`/`[capped]` marker in any tool names a file or a
next call; `bash {command, max_output_lines: 500}` honored and capped.

## 10. P7 — edit follow-through

1. C8 (existing row): `edit` becomes a freeform/grammar tool on
   openai-responses; JSON `function` shape stays everywhere else. This
   deletes the JSON-escaping tax on `+` body rows — the reference ships
   apply_patch exactly this way (Lark grammar, freeform).
2. Stale-tag rejection self-service: verify the mismatch context
   (`format_anchored_context`) renders enough fresh renumbered rows around
   the attempted anchors to retry without a `read`; widen if not. The
   philosophy stays the reference's — diagnose, never silently fuzzy-apply.
3. M3 (existing row): with P0's `errorKind`, report stale-tag rate,
   noop-loop rate, op mix (PUT range vs `N*` vs registers), and retry
   depth over a real corpus. NOOP_HARD_LIMIT came from one the reference incident;
   the next rule should come from these numbers.

done: C8's own done-gate; a stale-tag failure is recoverable in one turn
in the common case; M3's "instrumented stats reported" row closes.

## 11. P8 — highlight audit + fix

The scanner shipped without a corpus. Build the corpus first, then fix
what it convicts.

1. Fixture corpus per language under `crates/tui/tests/fixtures/highlight/`:
   real files from this repo plus adversarial cases — Rust: lifetimes
   (`'a`), char literals, raw strings `r#""#`, nested generics,
   SCREAMING_CASE; Python: triple-quoted strings, f-strings, decorators;
   shell: heredocs, `#` inside URLs and strings, `$()`; TS/JS: template
   literals, regex literals, JSX-ish angle brackets; JSON: escaped quotes.
   Snapshot the token-span stream with insta (already a dev-dependency).
2. Triage the diffs into: (a) real defects — the Rust `'` quote-set entry
   (highlight.rs:31) making every lifetime open an unterminated string is
   the presumed top hit; escaped-quote handling; `#` comment detection
   inside strings; (b) accepted limits of the per-line design — block
   comments and triple-quoted strings spanning lines. For (b), decide
   once: either document the limit in the module header, or thread a
   one-token carry state through the renderers that already walk lines
   top-to-bottom (markdown fences, diff bodies, pycell) — the state is one
   enum, not a parser, and D63's no-regex/no-dependency stance is
   untouched either way.
3. Fix (a); re-snapshot; the corpus becomes the regression gate.

done: corpus committed and green; every known miscolor either fixed or
named as an accepted limit in highlight.rs's header; comment budget
respected.

## 12. P9 — kitty orb stability

Today every repaint is: delete all placements of the id, re-transmit the
full deflated RGBA, re-place, cursor save/restore around it
(orb/kitty.rs:138-156) — and a placement *move* (transcript scrolled under
it) triggers the same full cycle (orb/mod.rs:152). Two windows of nothing
placed = flicker; full retransmit per scroll tick = bandwidth burned
exactly when the terminal is busiest.

1. Split transmit from placement. Transmit frame data with `a=t` (no
   display) under the session id; place with `a=p` and a fixed placement
   id. Kitty and Ghostty replace an existing (image, placement) pair
   atomically — no delete, no gap.
2. Retransmit data only when the animation frame advances (`due`); when
   only the cell position changed, re-place only (a placement escape is
   ~40 bytes vs the full frame's KBs).
3. Double-buffer ids on frame advance if in-place data replacement still
   shows a blink in either terminal: transmit new frame under id B, place
   B, delete A — place-before-delete, never the reverse.
4. Bracket the emit in the same synchronized-output update (mode 2026) as
   the ratatui draw that computed the placement cell, so the image and the
   text it sits beside land in one repaint. Verify term.rs's existing
   bracket covers the backend writes `tick()` makes directly.
5. Measure: bytes written per second during a fast-streaming turn, before
   and after (a counter in the Tick state, dumped by drive mode).

done: no visible flicker in ghostty and kitty while streaming scrolls the
transcript under the orb; per-frame cost during pure scroll is
placement-only; `orb::tick` behavior under `!supported()` unchanged.

## 13. P10 — markdown streaming newlines

Symptom: streamed prose reaches the reader in blank-line-sized batches;
inside a long fence or list, nothing commits until the block closes. The
newline gate exists (`commit_complete_source`, markdown.rs:15 — the reference
semantics, ported) but is dead code; `stable_cut` (blank-line gate) is the
only committer (app.rs:460). Design scrutiny, then the fix:

1. Audit the three-layer pipeline with a fixture that replays a recorded
   SSE stream through `App::reduce` and snapshots (committed lines, live
   tail) per event: (a) provider delta cadence (yi-ai sse.rs — is
   coalescing happening before `TextDelta`?); (b) commit granularity
   (`stable_cut`); (c) live-tail re-render each frame (render.rs:112-123).
2. Commit at newline granularity where the rendered prefix is provably
   stable, keeping the blank-line gate only where it is not:
   fence interiors are line-stable (each `│ code` row renders
   independently — commit them as they arrive; this is the biggest
   perceived-latency win, and it is exactly what the dead newline gate was
   ported for); paragraphs re-wrap as they grow, so paragraphs keep the
   blank-line gate; lists/tables keep the blank-line gate (the 0.43-era
   duplicate-item incident is the reason `stable_cut` exists — that
   invariant survives).
3. Fence-detection correctness in `stable_cut` while in there: `~~~`
   fences and 4+-backtick fences containing ``` examples currently toggle
   wrongly (markdown.rs:32).
4. Cost: stop cloning the full partial message per SSE delta where the
   clone is measurable (run.rs:325-345 clones every content block on every
   delta; `text_of` copies again at app.rs:527). Either carry deltas in
   the event alongside the partial, or coalesce MessageUpdate emission to
   the frame scheduler's cadence — measure first, the fix follows the
   number. Cache the live tail's rendered lines keyed on (tail bytes,
   width) so an unchanged tail costs zero per frame.
5. Delete `commit_complete_source` if step 2 subsumes it; a ported
   function with no caller is scaffolding.

done: streaming a 300-line fence commits line-by-line to scrollback;
lists/tables stream with no duplicated items (existing tui_unit tests
extended over the replay fixture); long-turn CPU profile flat where it was
quadratic.

## 14. Cross-repo rows (grid, ~/Development/grid)

Not this repo's phases; recorded here so the seam is designed once.

- `grid scope X --ranges`: emit the context pack as `path:start-end` rows
  (and in `--json`), directly consumable by P2's multi-range read.
- Auto-survey: any verb finding `.grid/` missing or stale runs survey
  first (~138 ms, idempotent) instead of failing — deletes the one
  ceremony step every session pays.
- Languages beyond Rust/Python stay deferred; the no-C TCB rules out
  tree-sitter, and `swc_ecma_parser` (pure Rust, heavy) waits until a TS
  repo matters. The survey/extract(Lang) seam keeps it pluggable.
- A first-class `grid` tool in Yi (schema'd verbs, tags minted on
  outputs) is deliberately not scheduled: bash + skill doc works, and P2
  wires the one integration that pays today.

## 15. Parking lot — ideated, not scheduled

Functional gaps:

- Eval harness: drive Yi headless through ref/benchmarks
  (terminal-bench-2-1, harbor, SWE-Atlas) and diff P0's stats across tool
  changes — the only way "the grep upgrade helped" ever becomes a number.
  Candidate first row after P0/P3 land.
- Image/document read: read returns text only; the reference/pi/the reference return
  images as attachments and M1 already covers docs conversion. Lands with
  a vision-capable default model, not before.
- Session-search tool (the reference's `session_search.rs` precedent): Yi's JSONL
  sessions are grep-able today; a tool-level search with pagination would
  make prior-session recall a first-class move. Pairs with the
  session-mining skill.
- Secret redaction M2 gets easier post-unban: `SecretObfuscator` over
  known shapes as a `RegexSet`.
- Glob results sort lexically (builtins.rs:179); mtime-descending puts
  active files first for the model. One-line change, measure token effect
  via P0 before adopting.
- ignore.rs: `.git/info/exclude` and global excludes unsupported — fine
  until a real repo bites; noted for the day one does.

Architecture / performance:

- MessageUpdate O(n²) clone chain (P10.4 measures it; if the number is
  big, the event type grows a delta field — a types change, so its own
  D-row).
- Best-of-N (M5) and auto-review (D1) rows stand; P0's stats give both
  their selection metric.
- `walk_files` is single-threaded; grep v2 may want a two-thread walk
  (the reference's comment: beyond 2 the walk is I/O-bound). Only if P3's benchmark
  says so.
- Kernel snapshot/journal (crates/kernel/journal.rs) already exists for
  busy-recovery; a `kernel.prewarm` + snapshot-revive fast path could make
  ipython effectively instant across sessions — measure demand via P0
  ipython counts first.
