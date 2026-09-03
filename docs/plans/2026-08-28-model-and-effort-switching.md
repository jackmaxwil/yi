# Model and reasoning-effort switching — a per-model effort ladder, a gated top tier, and the reference picker

```
status:  LANDED 2026-08-28 at 0.60.0 as D72 · U43, in one pass rather than
         four commits. A ponytail/§18 audit cut seven items and fixed four
         defects before implementation; the effort ceiling was then cut by
         directive (§7). Two things the plan did not name were found during
         implementation and are recorded in §8.
date:    2026-08-28
sources: four-agent survey of the §8.14/§12 donors (2026-08-28): the reference
         <ref>/tui/src/chatwidget/{model_popups.rs,reasoning_shortcuts.rs},
         <ref>/core/src/context/model_switch_instructions.rs · pi
         packages/ai/src/models.ts:900-932 · the reference packages/coding-agent/src/
         thinking.ts, session/model-controls.ts, modes/components/
         {model-picker,model-browser}.ts · the reference provider/transform.ts
         (read for the variant model, not adopted) · Yi ground truth:
         crates/types/src/{model,entry,config}.rs, crates/ai/src/{openai,
         openai_responses,catalog}.rs, crates/loop/src/{run,config}.rs,
         crates/runtime/src/{provider,session,subagent,compaction}.rs,
         crates/cli/src/{rpc,main}.rs, crates/acp/src/lib.rs, crates/tui/src/
         {keymap,status,render,app,agents,popup}.rs · YI_DESIGN §8.14 U-table,
         §12, §18, L10/R1/B1 · TODOS A5 · D34/D41/D43/D49/D55
```

## 1. Thesis

Yi can already *send* a reasoning effort and cannot *choose* one. The wire half
is done and correct — the bundled catalog carries pi's `thinkingLevelMap`, and
`crates/ai` renames and gates levels through it. Everything above the wire is
either a hardcoded global ladder, a lie, or absent.

Four borrowings, one directive, two refusals:

- **the reference** — the ladder is *per model, advertised by the catalog*. No global
  list, and a current effort the model does not advertise anchors rather than
  guessing a rung.
- **the reference** — the expensive tier is gated: the inline cycle key refuses to
  cross into it and names where it lives; the picker reaches it only through a
  second, explicit step.
- **pi** — `thinkingLevelMap` is the single table: it renames a level for the
  wire *and* decides whether the level exists (`null` = rejected,
  `xhigh`/`max` absent = unsupported). Yi already carries it; this plan makes
  the capability half reachable.
- **the reference** — the model-switching UI: a compact searchable picker with a thinking
  glyph per row, and its keybindings.
- **user directive** — the default effort is **medium**, always, unless config
  or the user says otherwise. Yi therefore needs no per-model `defaultLevel`
  field, which deletes machinery both donors carry.
- **refused** — the reference's prewalk and its `auto` per-turn classifier. The level
  changes when the user changes it and at no other time.
- **refused** — the reference's effort ceiling. It was in an earlier draft of this plan
  and is cut by directive. With no classifier and no prewalk, nothing in Yi can
  raise its own effort, so the ceiling's only possible writer was a new
  `config.thinking.max` key; without that key it is a parameter threaded
  through every clamp to constrain nothing. Cut with its writer.

**the reference's `<model_switch>` developer message is cited and skipped.** It exists
because the reference models carry divergent per-model instructions; Yi's system prompt
is per session (`crates/cli/src/main.rs:288`) and `yi_types::model::Model` has
no instructions field, so there is nothing to re-inject. Recorded so it is not
re-litigated; `Entry::ModelChange` is the hook if that ever changes.

**No fullscreen hub.** §8.14 bans the alternate screen and the reference's is 2,954
lines against 943 of headroom. Settled, not a tradeoff to revisit.

Six items, four phases.

| phase | items | what |
|---|---|---|
| P0 | 1 | `Effort` and the per-model ladder — one home, no second ladder |
| P1 | 2 | the level becomes turn-scoped; the shared-provider bug dies |
| P2 | 3, 4 | every entry point clamps; resume restores; the old ladders die |
| P3 | 5, 6 | the TUI: picker with effort step, cycle key, status line |

P0 lands alone with no visible change. P1 is the only phase touching
`yi-loop`'s trait. P3 is the only phase spending TUI budget.

## 2. Ground truth (verified 2026-08-28)

**Already right, do not rebuild:**

- `crates/ai/data/*.json` carries `thinkingLevelMap` with pi's semantics —
  `claude-fable-5` is `{"off": null, "xhigh": "xhigh", "max": "max"}`: cannot
  disable thinking, does advertise the top two tiers.
- `crates/ai/src/openai.rs:302-326` — `nearest_effort` / `mapped_effort`, the
  wire rename plus `null`-rejection, cited against
  `scripts/openrouter_reasoning.py`. Used by `openai.rs:328`,
  `openai_responses.rs:222`.
- `yi_types::entry::Entry::{ModelChange, ThinkingLevelChange}` — the transcript
  shapes exist, serialized, under the Pi v4 byte-compat contract.
- `crates/tui/src/status.rs:74` renders ` · ◉ {thinking}` when given one.
- `crates/runtime/src/compaction.rs:180-188` — `due()` measures against the
  **current** model's `context_window` every turn, so switching to a
  smaller-window model self-corrects on the next turn. Nothing to add.
- `crates/runtime/src/provider.rs:48-61` — `anthropic_thinking`'s `_ => 16384`
  budget arm looks wrong for `xhigh`/`max`, and is unreachable: every anthropic
  model in the catalog advertising those tiers sets `forceAdaptiveThinking`, so
  it takes the Adaptive branch, and after P0 no clamp can hand the Budget
  branch a tier the model never advertised. Left alone.

**Wrong or missing:**

- `crates/cli/src/rpc.rs:14` — `THINKING_LEVELS: [&str; 5]`, a global ladder,
  missing `xhigh`/`max`, unrelated to what any model advertises. 129 models in
  the bundled catalogs advertise a tier this list cannot name.
- `crates/cli/src/rpc.rs:295` — `"thinkingLevel": "off"` hardcoded in
  `get_state`. Every RPC and ACP client is told the wrong thing.
- **Bug.** The level lives on `ProviderStream` (`provider.rs:67`),
  `AgentSession::new` writes it there (`session.rs:110`), and
  `subagent.rs:1032` hands every child `Arc::clone` of the **same** object.
  Spawning a subagent with `thinking="low"` rewrites the parent session's
  effort for the rest of its life, unobservably.
- `crates/loop/src/config.rs:19` — `NextTurn.thinking: Option<String>` is in the
  L10 contract; `run.rs:477` reads only `next.model`. Dead field.
- `crates/runtime/src/session.rs:345-368` — `attach_store` restores messages
  only. Model and effort do not survive `--continue`.
- No TUI surface: `SLASH_COMMANDS` (`app.rs:74`) has no `model`;
  `render.rs:166` passes `thinking: None`; `keymap.rs` has no model or effort
  action. TODOS `A5` names it.
- `UserConfig` has no thinking key; `--thinking` (`main.rs:68`) is an
  unvalidated free string.

**Budgets:** `yi-tui` 9,057 / 10,000 (D43) — **943 lines of headroom**. File
ceiling 1,200, function ceiling 150.

## 3. Cross-cutting decisions

**3.1 One ladder, derived, in `yi-types`.** `Effort` is an exhaustive enum in
`yi_types::model` — `Off, Minimal, Low, Medium, High, XHigh, Max` — and the
only ordered list of levels in the repo lives beside it. Everything else asks
`supported_efforts(&Model) -> Vec<Effort>`, pi `models.ts:902-911` ported
verbatim:

```
not model.reasoning         -> [Off]
level mapped to null        -> excluded  (the model rejects it)
XHigh | Max absent from map -> excluded  (opt-in tiers)
otherwise                   -> included
```

`yi-types` is the DTO wall (§2: no fs, no `$HOME`) and these are pure functions
over the DTO, the same shape as `Entry::id()`. Putting them there is what lets
`yi-ai`, `yi-runtime`, `yi-cli` and `yi-tui` share one ladder without a
re-export chain (`boundaries.toml` forbids `yi-tui -> yi-ai`).

**Invariant: the returned list is never empty** — a reasoning model whose every
level is `null`-mapped falls back to `[Off]`. Callers still use `.first()` /
`.last()` with `unwrap_or(Effort::Off)`; nothing indexes it (§18 panic budget
is zero, not ratcheted).

**`Effort` carries no `Other(String)`** despite §18's wire-facing rule. It never
crosses a wire as an enum: the Pi v4 entry field is `thinking_level: String`
and RPC, ACP, config and CLI all parse at the boundary. Internal enums are
exhaustive, which is what makes a new rung a compile error everywhere.

**3.2 One clamp.** `session_clamp(model, effort)` snaps within
`supported_efforts` — up before down (pi `models.ts:913-932`), so a user never
silently gets less thinking than they asked for while a higher supported rung
exists. Two arguments, one list, one pass.

`nearest_effort` in `yi-ai` solves the same problem for efforts that never
passed a clamp. Once P2 clamps every entry point it is unreachable, and P2
deletes it. Net deletion, not duplication.

**3.3 The default is medium, and it is clamped like everything else.** A fresh
session starts at `session_clamp(model, Medium)`. A model switch keeps
the current effort when the new model advertises it and clamps when it does not
— it never consults a per-model default, because there isn't one. This is why
Yi skips the reference's `defaultLevel` re-application (`model-controls.ts:562`) and pi's
`_getThinkingLevelForModelSwitch` (`agent-session.ts:1775`) entirely.

**3.4 A level change takes effect at the next turn boundary.** the reference's rule
(`model_switching.rs:528`), and it falls out of P1: the run loop reads the
effort at turn start and `prepare_next_turn` re-reads it, so a mid-stream
change lands on the following turn instead of tearing a request in half.

**3.5 Write and restore, or neither.** `Entry::{ThinkingLevelChange,
ModelChange}` are appended on every effective change *and* replayed by
`attach_store`. Writing entries nothing reads is write-only data; restoring
without writing is impossible. They land in the same step.

**3.6 The port brings behavior, not prose.** §18/D49: no comments by default,
three lines hard, `Incident:`/`Invariant:` tags only, referents as intra-doc
links (D55). the reference's `reasoning_shortcuts.rs` opens with a 15-line module
essay; it does not come over. The one comment this work earns is the
`Invariant:` on `supported_efforts`' non-empty guarantee.

## 4. Phases

### P0 — the ladder (item 1)

`crates/types/src/model.rs`: `Effort`, `Display`, `FromStr`,
`supported_efforts`, `session_clamp`. `thinking_level_map` stays
`Option<Value>` — typing it as a map touches the catalog parser and sixteen
fixtures to buy nothing the accessor gives.

`FromStr::Err` carries the offending string and nothing else (§18: variants
carry the value, not a rendered message). The advertised list belongs to the
*caller's* error text, because `FromStr` has no model in scope — a parse
failure and an unsupported-for-this-model failure are different, and P2's call
sites report them differently.

`yi-ai`'s `mapped_effort` starts taking `Effort` instead of `&str`.

**Gate:** a table test over all three bundled catalogs asserting
`supported_efforts` per model — `claude-fable-5` excludes `Off` and includes
`Max`; a non-reasoning model yields exactly `[Off]`; the list is never empty.
No user-visible change.

### P1 — turn-scoped state (item 2)

`StreamFn::stream` gains an `effort: Effort` parameter (`run.rs:19-26`; call
sites `run.rs:292`, `compaction.rs:64`, `provider.rs:100`). `run.rs` carries
`current_effort` beside `current_model`, seeded from config and updated by
`prepare_next_turn` — which retypes `NextTurn.thinking` to `Option<Effort>` and
finally reads it. `ProviderStream::{thinking_level, set_thinking_level}` are
deleted.

*Rejected alternative:* adding `effort` to `LlmContext` needs no trait change,
but `LlmContext` is a serialized `yi-types` DTO, so a field there is a §19
schema change — and effort is a request knob, not context. The trait parameter
puts effort exactly where `model` already is, which is the honest shape; the
mutable shared field was the accidental complexity.

`AgentSession` gains `effort: Mutex<Effort>`. `set_effort` clamps per §3.2,
appends `Entry::ThinkingLevelChange`, emits, and **returns the effective level**
so callers report what was set rather than what was asked (§18: no infallible
signature over a fallible operation). `set_model` re-clamps per §3.3 and appends
`Entry::ModelChange`.

**Gate:** the regression this phase exists for — a parent at `high` spawns a
child at `low`; the parent is still at `high` afterwards. Plus: a `set_effort`
during a running turn is absent from that turn's request and present in the
next (§3.4).

### P2 — every entry point clamps (items 3-4)

**Item 3 — surfaces stop lying.** `rpc.rs`: delete `THINKING_LEVELS`;
`get_available_thinking_levels` returns `supported_efforts(&session.model())`;
`set_thinking_level` clamps and replies with the effective level; `get_state`
reports the real one. `acp/src/lib.rs:415-419` routes through the same session
methods. `subagent.rs:425` parses `thinking` into an `Effort` and clamps against
the child's model — an unsupported rung is clamped, not an error, because the
kwargs come from a model and failing a spawn over a rung is the wrong trade.
`UserConfig.thinking: Option<String>` parsed strictly at load (X7 already errors
on typos). Precedence: `--thinking` > project config > user config > `Medium`.
Then delete `nearest_effort` (§3.2).

**Item 4 — resume restores.** `attach_store` scans the branch for the last
`ModelChange` / `ThinkingLevelChange` and applies them through the same clamp
(§3.5). Fixture-check the written entries against Pi's own storage output
before landing (§19, D32).

**Gate:** an unknown effort in `~/.yi/config.json` names the value and exits
non-zero; `yi --continue` comes back on the model and effort it left on; a
subagent spawned with a rung its model does not advertise gets the clamped one
and leaves the parent's untouched; the RPC level list changes when the model
changes.

### P3 — the TUI (items 5-6)

Budget: **≤ 260 new lines in `yi-tui`** against 943 of headroom. Named up front
so the phase is measured, not discovered.

**Item 5 — the picker (`crates/tui/src/model.rs`, ~170 lines).** A `Bottom`
variant built the way `agents.rs` is built — `BottomView` plus the filter idiom
from `ListPopup::filtered` — with the reference `model-picker.ts` as **read-only
reference for the row anatomy only**: `provider/id`, thinking glyph, current
model marked, ordering by recency then provider. `ListPopup` itself is not
reused: its rows are plain `String` with no per-row styling.

Selecting a model whose ladder has more than one rung replaces the body with
the effort list — the reference's chained popup, in place rather than as a second
overlay. Rungs above `High` are not listed; a trailing `More reasoning…` row
reveals them, which is the reference's gate (`model_popups.rs:521-545`) with its
two-step shape intact.

**Item 6 — keys and the status line (~90 lines).** the reference's bindings, all free in
Yi's keymap: `shift-tab` cycles effort, `ctrl-p` / `shift-ctrl-p` cycles the
model, `alt-m` opens the picker, `/model` opens it from the composer. The
effort cycle walks `supported_efforts` only, **never enters `XHigh`/`Max`**, and
at the boundary says where they live — the reference `reasoning_shortcuts.rs:117-145`,
ported whole because it is the entire value of the gate. `ctrl-p` cycles the
session's MRU models (the config `model`, plus anything picked this session);
no scope config, no `--models` flag — if an explicit scope is wanted later it
is a `models.cycle` field and ten lines. `render.rs:166` passes the real level
into `StatusInput.thinking`.

**Gate:** a drive script (`drive.rs` grammar) opening the picker, filtering,
selecting a model, picking an effort, asserting the status line; a unit test
that `shift-tab` from `high` on a `max`-capable model stays at `high` and
commits the hint; a frame assertion on the picker at 80 columns.

## 5. Appendix A additions

| item | source span | lines | action |
|---|---|---|---|
| supported-level derivation + clamp | `ref/agents/pi/packages/ai/src/models.ts:900-932` | 33 | port verbatim |
| inline effort shortcut: anchor, walk, refuse-to-cross | `ref/agents/<ref>/tui/src/chatwidget/reasoning_shortcuts.rs:55-200` | 146 | port adapted (prose dropped, §3.6) |
| chained effort popup + `More reasoning…` gate | `ref/agents/<ref>/.../model_popups.rs:456-560` | 105 | port adapted |
| compact session picker — row anatomy | `ref/agents/<ref>/.../model-picker.ts:1-237` | 237 | **read-only reference** — built on `agents.rs`'s shape instead |
| **effort ceiling clamp** | `ref/agents/<ref>/packages/coding-agent/src/thinking.ts:283-330` | 48 | read-only reference — cut by directive (§1) |
| **`<model_switch>` injection** | `ref/agents/<ref>/.../model_switch_instructions.rs` | 37 | read-only reference — deliberately not ported (§1) |
| **fullscreen `/models` hub** | `ref/agents/<ref>/.../model-hub.ts` | 2,066 | read-only reference — §8.14 bans the alternate screen |
| **prewalk hand-off** | `ref/agents/<ref>/.../session/prewalk.ts` | — | read-only reference — refused by directive |
| **auto-thinking classifier** | `ref/agents/<ref>/.../model-controls.ts:592-650` | — | read-only reference — refused by directive |

## 6. Risks

- **P1 touches `yi-loop`'s public trait.** Three call sites, but `StreamFn` is
  the L-table contract; the signature change wants its own D-row.
- **The 943-line TUI headroom.** If P3 overruns, the cut is the picker's
  provider grouping, then the glyph column — never the effort gate, which is
  the borrowed value.
- **Pi v4 byte-compat.** P2 starts writing two entry types Yi has only ever
  read.

## 7. What the audit cut (2026-08-28)

Recorded so none of it comes back as "we already planned that":

| cut | why |
|---|---|
| the effort ceiling: `config.thinking.max`, `AgentSession::ceiling`, the ceiling arg on `session_clamp`, its subagent inheritance and its gate | **user directive.** The audit had already found it had no writer and proposed the config key as its one justification; the key is not wanted, so the ceiling goes with it rather than being threaded through every clamp to constrain nothing |
| Anthropic budget-table extension for `xhigh`/`max` | unreachable: every anthropic model advertising those tiers is `forceAdaptiveThinking`, and after P0 no clamp can hand the Budget branch an unadvertised tier |
| over-context gray-out + compact-first on model pick | `compaction.rs:188` already measures against the current model's window every turn |
| `models.cycle` config + `--models` flag + `⟳n` badges | MRU is the scope; the field is ten lines the day it is actually wanted |
| a guardrail forbidding a second effort ladder | the enum is the guardrail; a regex over `"minimal"`/`"xhigh"` would fire on `yi-ai`'s legitimate wire strings |
| `/thinking [level]` slash command | third surface for one setting; `/model` + `alt-m` + `shift-tab` cover discovery and speed |
| cost hints in picker rows | speculative; nothing asked for them |
| porting `model-picker.ts` as 237 lines | `agents.rs` is the same shape already in the crate |

Three defects fixed: `FromStr` cannot report a model's advertised list (no model
in scope); entries were written with nothing restoring them;
`NextTurn.thinking`'s retype was listed in two phases. A fourth — the
sequential clamp snapping up past the ceiling — died with the ceiling.

## 8. Found during implementation (not in the plan)

Two things the ground-truth pass missed, both made live by this work rather
than created by it:

| found | why it mattered now | what landed |
|---|---|---|
| `SubagentHostOptions.default_model` is a snapshot taken at host construction, and `model.info` reads the same snapshot | dormant while nothing could switch models; the moment P3 gave the TUI a picker, spawning a child after a switch would use the startup model and `model.info` would report it | replaced by a live `defaults: Arc<dyn Fn() -> (Model, Effort)>` handle, so a child inherits what the parent is actually on. `Shared` gains `model`/`effort` so the handle can read them |
| `crates/acp/src/lib.rs:202` carried a *second* hardcoded five-level ladder in `config_options`, and reported `"value": "off"` unconditionally | the plan named `rpc.rs`'s ladder and missed this one; deleting one and leaving the other would have kept the defect under a different roof | both derived from `model.supported_efforts()` and `session.effort()` |

One deliberate behavior change the plan did not call out: **compaction now
summarizes at `clamp_effort(Off)`** rather than inheriting the session's level.
It only ever inherited it through the shared-`ProviderStream` bug this work
deletes, summarization is mechanical, and threading the session's effort into
`complete_text` would have been plumbing to preserve an accident.
