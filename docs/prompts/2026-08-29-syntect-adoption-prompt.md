# Handoff prompt: plan syntect adoption (supersedes D63)

Paste the block below as the first message of a fresh session, from the
`tui-visual-upgrade` worktree or a new branch off it.

---

Yi currently highlights code with a hand-rolled per-line scanner in
`crates/tui/src/highlight.rs` (U39, D63). That decision was taken on a size
budget. The owner has now **accepted syntect with its full bundled syntax set,
including the binary and dependency cost**. Your job is to produce a
comprehensive implementation plan in `docs/plans/`, not to implement it.

Read first: `docs/YI_DESIGN.md` (§8.14, §13.5, U39), `docs/ARCHITECTURE.md`
(D63 and the 0.48.0 changelog), `docs/solutions/adr/d63.md`,
`docs/size-ledger.md`, `deny.toml`, `scripts/guardrails/`.

## The decision you are planning under

Adopt `syntect` 5.3 with the default 75-syntax set. Accept the binary growth and
the transitive crates. Two constraints ride along, and the plan must hold both:

- **Minimize the bloat within that decision.** Accepting the cost is not
  accepting the default feature set.
- **Optimize the runtime.** Yi's TUI redraws its live region on an 80 ms tick.

## What the measurements already say

All figures below come from probe crates built on Yi's own `dist` profile
(`opt-level="s"`, `lto="fat"`, `codegen-units=1`, `panic="abort"`,
`strip="symbols"`), each one loading a syntax set **and running one
`parse_line`**, against a 286,048-byte do-nothing baseline:

| probe | binary | delta |
|---|---|---|
| baseline | 286,048 | — |
| syntect 5.3, default features, 75 syntaxes | 1,758,384 | +1.40 MiB |
| syntect 5.3, zero syntaxes (code alone) | 1,395,168 | +1.06 MiB |
| synoptic 2.2.9 | 1,295,632 | +0.96 MiB |
| current hand-rolled scanner | — | +16,544 bytes |

An earlier probe reported syntect at +0.38 MiB. It loaded the syntax set and
printed a count without ever parsing, so LTO stripped the parser it never
called. **Every probe you build must exercise the code path it is buying**, or
you are measuring the linker.

Yi's `dist` binary is currently 6,041,392 bytes against a 6,291,456-byte
ceiling.

## Work the plan must cover

**1. Feature selection — the main size lever left.** The +1.40 MiB figure is
syntect's *defaults*, which include `default-themes`, `html`, `plist-load` and
`yaml-load`. Yi needs none of them: it has its own `Theme` with a
`ColorTier` ladder, and the 75 syntaxes ship as a compressed bincode dump, not
as YAML. Propose a `default-features = false` set — verify the exact feature
names against syntect 5.3's own `Cargo.toml`, do not trust this list — around
`parsing`, `default-syntaxes` (which pulls the dump loader) and a regex backend.
Measure the trimmed set the same way and put the number in the plan.

**2. The regex backend, which is the real dependency question.**
`regex-onig` links oniguruma, a C library: §13.5 bans `syntect` **with onig**
by name, and it would break the offline/cross-compile story. `regex-fancy` uses
`fancy-regex`, which depends on `regex-automata` and `regex-syntax` rather than
on the `regex` crate itself — **verify this with `cargo tree`**, because
`deny.toml` denies `regex` outside a `zeromq` wrapper, and `multiple-versions =
"deny"` means a `regex-automata`/`regex-syntax` version that collides with the
existing tree is a hard failure, not a warning.

**3. Use `ParseState` and `ScopeStack`, not `HighlightLines`.**
`HighlightLines` requires a syntect `Theme`, which would drag in the theme dumps
and put colour policy outside Yi's tier system. Parse to scopes and map
`Scope` to Yi's existing `highlight::Token` enum yourself. Keep
`Theme::syntax_style`'s conversion policy unchanged: **foreground and bold
only** — a background would fight the diff tint the row renders inside.

**4. Multi-line state is the whole point of the change.** The current scanner is
per-line, so a block comment or a multi-line string mis-colours after its first
line. `ParseState` carries state across lines. The public API therefore has to
change shape: today `highlight::spans(line, lang, theme, base)` is stateless and
per-line. Plan the replacement as something constructed per file or per cell and
fed lines in order. Callers to migrate: `diffview.rs`, `pycell.rs`, `cell.rs`
(the `bash` summary row), `markdown.rs`. Note that `diffview` feeds lines
**out of source order** (a hunk skips lines, and `-`/`+` rows interleave two
versions) — say explicitly what state a diff row gets, because "resume the
parse state from the previous rendered row" is wrong there.

**5. Startup and per-frame cost.**
- `SyntaxSet::load_defaults_newlines()` decompresses a dump. It must not run on
  the startup path: `scripts/guardrails/check_startup.py` holds
  `yi --version` to 5.0 ms. Plan a `OnceLock` initialised on first use, or a
  background thread at TUI init — and measure, don't assume.
- Committed scrollback rows are rendered once, so they already pay once. The
  **live region re-renders every 80 ms tick**: a running `ipython` cell with an
  open source body, or a live diff, would re-parse on every frame. Decide
  whether spans are cached per cell and invalidated on content change, or
  whether live bodies keep the cheap path. Put a number on it.
- Keep `LINE_CAP = 4_096`. A generated line is where a regex engine is slowest.

**6. Guardrails and docs.** Every one of these is part of the plan, with the
new number where you can measure it and a `--update` step where you cannot:
- `scripts/guardrails/baselines/binary_size_budget.json` (`max_bytes`, now
  6,291,456) and `deps_budget.json` (`transitive`, now 167).
- `deny.toml`: remove `"syntect"` from `bans.deny`; check whether anything new
  needs a `skip` entry under `multiple-versions = "deny"`.
- `docs/YI_DESIGN.md` §13.5 (the ban line) and the U39 row at ~line 1001.
- `docs/ARCHITECTURE.md`: version bump, changelog row, and a new D-row that
  **supersedes D63** rather than editing it.
- A new ADR under `docs/solutions/adr/` plus its `docs/solutions/README.md`
  index line.
- `docs/size-ledger.md`: a row with the real before/after.
- `docs/TODOS.md`: A7 is closed against the scanner; reopen or amend it.

**7. Tests.** `crates/tui/tests/highlight.rs` has 7 tests written against the
hand-rolled scanner's behaviour — including that an uppercase-initial word is a
`Type` and a word followed by `(` is a `Function`, both of which are heuristics
syntect replaces with real scopes. Say which expectations change and add
multi-line coverage (block comment, triple-quoted string, a diff hunk that
opens inside one).

## Ground rules

- Skills: HAR, caveman lite, ponytail lite.
- The plan is a document, not code. Land it in `docs/plans/` and stop.
- If a measurement contradicts something written above, the measurement wins —
  correct the text in place and say what was wrong, the way `docs/size-ledger.md`
  records the D63 correction.
- If the trimmed feature set still lands somewhere the owner should weigh in on
  (say, over 8 MiB, or a `multiple-versions` collision with no clean skip),
  surface it as a decision rather than choosing silently.
