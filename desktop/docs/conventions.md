# Conventions

Short rules every task follows. `docs/architecture.md` says what the pieces are; this says how to write them.

## What you may touch

- Never edit a `Cargo.toml`, a `lib.rs`, a `mod.rs`, a registry (`panes/src/registry.rs`, `sources/src/registry.rs`, `ui/src/scenes.rs`, `app/src/scenes.rs`), `core/src/model.rs` or `core/src/msg.rs`. If you need to, stop and say so in your report.
- A module the tree lacks goes under a file you own: `pane.rs` declares `mod rows;` and you add `pane/rows.rs`.
- A stub you do not need stays as it is. Do not delete files.

## Naming

- Crates are `yd-<name>` in `crates/<name>`; the program is `yi-desktop` in `crates/app`.
- Modules and files are one full word where one exists: `tabs`, `rows`, `state`. No `util`, `helpers`, `common`, `misc`, `manager`. No abbreviations a newcomer must decode.
- A pane has one slug (`PaneKind::slug`): `chat`, `code`, `lsp`, `plugin-kit`. Its module folder is the slug with `_` for `-`, except `code_view` (slug `code`) and `script_pane` (slug `script`).
- Types say what they are without the module's name: `grid::layout::Layout`, not `GridLayout`.
- Ids and units are newtypes from `yd_core::ids` (`ThreadId`, `RowId`, `PaneId`, `FeedId`, `CommandId`, `Millis`, …). No bare `String` or integer id crosses a crate boundary. A new id is a newtype in the file that owns the thing.
- Method prefixes carry cost: `as_` is free, `to_` allocates, `into_` consumes. Getters have no `get_`.

## Module layout

```
crates/panes/src/<pane>/
  mod.rs      frozen: the module list and `pub use pane::DEF`
  pane.rs     the definition, the view, the scenes, the pane's commands and settings
  desc.rs     the list description, where the pane is a list
  states.rs   empty, loading, offline and error states (task QS)
crates/ui/src/<engine>/
  mod.rs      frozen
  view.rs     the engine's entry point and its `SCENES`
  state.rs    the state a pane keeps for the engine
crates/core/src/<part>/
  mod.rs      frozen
  …           the part's model type, its message type and its `step`
```

Files stay under 800 lines. Split at a seam, not at a line count. `rustfmt` breaks a builder chain past 60 characters into a line a call, so long chains eat the budget: keep chains short and name the parts with `let`.

## Keys

Every element a user can point at, a test finds, or a box is drawn round has a stable key.

| Element | Key | Made by |
|---|---|---|
| A pane's root | `<slug>-<pane id>`, such as `agents-3` | `PaneCx::key(kind)` |
| Something inside a pane | `<pane key>.<part>`, such as `agents-3.pane-header` | prefix with the pane key |
| A pane's tab bar, a tab, its meta, its `+`, and the split, pull-out and close icons | `<pane key>.pane-tab-bar`, `.tab-<tab id>.pane-tab`, `.pane-meta`, `.pane-tab-new`, `.pane-split`, `.pane-out`, `.pane-close` | the chrome alone: a pane never draws a bar |
| A pane's header bar, the column titles, the footer line | `<pane key>.pane-header`, `.table-header`, `.footer` | the list engine, and any pane with the same bars |
| A list's scroll viewport (takes the list's keys) and its selection bar | `<pane key>.rows`, `<pane key>.cursor` | the list engine |
| A row | `<pane key>.row.<row id>` | the list engine |
| A group heading | `<pane key>.group.<id of the group's first row>` | the list engine |
| An open row's detail, and a button in it | `<pane key>.detail.<row id>`, `<pane key>.detail.<row id>.<command id>` | the list engine |
| A value's row in the theme editor, and its control | `<pane key>.values.row.<name with dots as dashes>`, `<pane key>.value.<name>.set` | the theme editor |
| A list that is one part of a pane | the same keys under `<pane key>.<name>`, such as `lsp-3.servers.row.<id>` | `list::view::part` |
| A code line | `<pane key>.L<n>` | the code surface |
| A kit component | `kit.<name>` or the key its caller gives it, such as `kit.button.plain` | the kit |
| An own tab's close cross, and the room for the traffic lights in a bar that is a title bar | `<pane key>.tab-<tab id>.pane-tab-close`, `<pane key>.pane-lights` | the chrome |
| The remote screen, its letterbox, its picture, a window or row drawn over it | `<pane key>.screen`, `.letterbox`, `.surface`, `.screen.window:<id>`, `.screen.item:<id>` | the Remote desktop pane |
| The tray above the composer | `<pane key>.context-tray` | `chat::tray::view` |
| A prompt, an answer and a tool step in the chat pane | `<pane key>.message.<n>.bubble`, `.message.<n>.answer`, `<pane key>.step.<call id>` (the document: `.block.answer.<n>`); found as `yi://message`, `yi://answer` and `yi://step` things once the server has stored them | `chat::pane::finder` |
| The composer, in the pane, the document and New chat | `<pane key>.composer`, `.composer.input`, `.composer.send`, `.composer.model`, `.composer.mode` (`new-chat.composer.*` in New chat) | the one composer |
| The overlay | `overlay`, `overlay.base` (the window's body), `overlay.boxes`, `overlay.box.<address>`, `overlay.pick`, `overlay.preview` | `overlay::boxes::over` |
| The design box over a web page | `<web key>.design`, `.design.element` (with `.number` and `.tag`), `.design.handle.<nw to w>`, `.design.size`, `.design.gap`, `.design.inset`; the switch is `<web key>.mode` with items `.0` to `.2` | `design::handles::layer`, the web bar |
| The Design pane | `<pane key>.design-header`, `.pick-another`, `.row.<property>`, `.value.<property>` (padding's second box `.1`), `.field`, `.refused`, `.changes`, `.change.<element number>.<property>`, `.hand`, `.reset`, `.note`, `.state` (nothing picked) | `design::pane` |
| Mark-up over a screen | `<surface key>.marks`, `.mark.<n>` (with `.number`), `.mark.<n>.note`, `.mark.<n>.note.field`, `.mark.new` while a region is dragged out | `design::markup::layer` |
| The window's menus and pages | `window.command-box`, `window.context-menu`, `window.title` (the title row of Just chat and New chat), `new-chat.<part>`, `notifications.<part>` | their owners |
| Shell and window pieces | `shell.<part>`, `window` (a pane's window's root), `root` | their owners |

The parity check finds a size part by key: the key equals the part or ends with `.part`; the part `row` matches any key holding `.row.`, and `code-line` any key ending `.L<n>`. A part that is drawn needs a key of that name, or its size is never checked. A pane part is never named `row`: `.row.` is how a picked key finds its row (`list::row::id_of`).

A key is built from ids, never from an index or from display text. A `memo` id is one name in the whole window, so it carries the pane key too; use the same string for a row's key and its memo id. Handlers carry the row id, never its position.

## Scenes

- A scene is `Scene { name, stage, setup }`. The name is `<pane slug>/<state>` in lower case with `-` between words: `chat/approval-waiting`, `logs/error-open`, `shell/everything`. Engines use their module name: `list/open-row`, `kit/gallery`. A parity scene of something that is not a pane is named after its fixture: `context-menu/mockup`, `context-tray/mockup`.
- `stage` says what the window draws: `Stage::Pane(kind)` (that pane alone under its tab bar, as the grid draws it), `Stage::View(fn)` (an engine's demo or a whole-window form) or `Stage::App` (the shell with the grid in it).
- A pane's scene sets the pane's state under `SCENE_PANE` and brings its own thread only to a hub that has none. A window's scene (`shell/everything`) runs several panes' `<slug>/mockup` setups over one thread and hands each pane's state to the grid's pane with `hub.panes.adopt(SCENE_PANE, pane)`. A scene adds rows to a table another pane also fills with `Upsert`, not `Reset`.
- **Scene state comes from fixtures through the real path.** `setup` gets the hub and builds state by sending events through `step`: read a fixture with `include_str!`, turn it into rows with `yd_core::row::rows_from_json` or replay recorded wire lines, and apply them. Set a pane's own state with `hub.panes.get_mut`. Never write model fields directly and never invent data in a view.
- **The parity scene.** A pane that the mockup draws has a scene named `<slug>/mockup` showing exactly the mockup's state (the same rows, the same row open). `just scenes` checks its element tree against `fixtures/mockup/panes/<slug>.text.json`: at least 97% of the mockup's on-screen text must be there, minus what `fixtures/mockup/intended.json` lists with a reason. A fixture read from one part of a pane (`settings-groups`, `findings-gate`) has a scene of that name drawing the same pane, and `tools/parity/parity.py` (`PANE_ROWS`) says which keys are that part's lines. `shell/everything` is also checked whole against frame 1. `docs/parity.md` is the report of every frame; an exception that says "not built" is listed there under what remains.
- Every pane kind keeps at least one scene. Replace `<slug>/stub` with real states when you build the pane; keep the name of the first state short and plain.
- `just scenes` writes `target/scenes/<pane>/<state>.png`. Look at the picture beside the mockup still before you call a task done. Headless pictures never show glass: they show the root's opaque fill.

## Fixtures

`python3 tools/fixtures/extract.py` writes everything under `fixtures/mockup/` from the built mockup with headless Chrome; the output is byte-identical on a second run, nothing in it is typed by hand and none of it is edited. `fixtures/mockup/README.md` is generated beside it and lists every list pane's field names and cell kinds; read it before writing a `ListDesc`.

| Kind | Path | Shape |
|---|---|---|
| Rows of a list pane | `fixtures/mockup/<slug>.rows.json` | A JSON array of rows: `{"id", "parent"?, "group"?, "dim"?, "tone"?, "cells": {"<field>": <value>}}`. A field is the column name in lower case with `_`; a detail fact whose label repeats a column's gets the suffix `_detail`. A row id is a slug of the displayed name (a log line's is its time). |
| A cell value | inside a row | One of `{"text": "…"}`, `{"dot": "run"}`, `{"badge": ["High", "bad"]}`, `{"pill": ["New", "context"]}`, `{"meter": {"ratio": 0.41, "text": "41%"}}`, `{"tags": ["a"]}`, `{"bar": {"start": 0.1, "len": 0.4, "tone": "model"}}`, `{"flag": true}`, `{"series": [1, 2]}`. A coverage cell is a flag or the text `–`. |
| A group's summary | the cell `group.summary` on the group's first row | Drawn beside the group heading. The extractor keeps the mockup's text in `meta.json` under `group_notes` and does not write the cell. |
| Which row is open or selected | `fixtures/mockup/meta.json` | Per list pane: `open`, `selected`, `group_notes` |
| The page's own tables | `fixtures/mockup/data/<NAME>.json` | A table under the page's name (`AG`, `LG`, `FD`, …) and the parts it builds inline (`settings.json`, `hub.json`, `icons.json`, …) |
| On-screen text of a pane or frame | `fixtures/mockup/panes/<slug>.text.json`, `frames/<n>.strings.json` | `{"pane", "frame", "strings", "titles"}`; one string per block of text |
| Measured sizes of a pane or frame | `fixtures/mockup/panes/<slug>.sizes.json`, `frames/<n>.sizes.json` | `{"sizes": [{"part", "check", "height", "width", "padding", "font_size", "line_height", "selector"}]}`; `check` lists what parity compares (height, width) |
| Themes | `fixtures/mockup/themes/<id>.json` (`yi`, `graphite`, `ink`, `contrast`, `terminal`) | `{"id", "name", "note", "dark", "values": {"<token name>": "#rrggbb" or "rgba(r, g, b, a)" or number or text or true/false}}` |
| What a scene may leave out | `fixtures/mockup/intended.json` | Entries `{"pane" (or `"*"`), "string" or "contains" or "size", "reason"}`; each entry carries its reason |
| Recorded wire lines | `fixtures/wire/<name>.jsonl` | One frame per line, either direction, as recorded: `{"dir": "send" or "recv", "frame": …}`; `fixtures/wire/README.md` says how each was made |

Dots are `need ask work done idle run stop fail pause`. Tones are `fg dim faint accent need ok bad warn model context`. A fixture comes from the mockup or from a real server, never from this app's own output.

## Theme values

Every colour a view draws is a theme value; `just colours` fails on a colour literal in a view, outside the exact files `tools/gate/colours.py` lists with their reasons.

- The shared table is `yd_core::tokens::Tokens` (`Tokens::NAMES`). A colour it lacks is a named value in `yd_ui::theme::scale`, an `Ink("<name>", <Yi Dark's colour>)` read with `.of(tokens)`, listed in `INK_GROUPS` under the editor group it shows in and set in `theme/yi-dark.json`. Two literals that mean the same thing share one value; a new one starts at today's colour, so no picture changes.
- `look::NONE` is a fill or line that is not drawn; no theme colours it. `white(tokens.lines)` and `white(tokens.inner_lines)` are the record's hairlines, whose strength a theme sets.
- The named colours, by editor group:

| Group | Names |
|---|---|
| Colour roles | `pill.fixing` |
| Text | `text.on_primary`, `text.on_accent`, `text.on_ok`, `text.on_model`, `context.on`, `context.tag`, `context.preview`, `text.on_note` |
| Surfaces | `fill.card`, `fill.menu`, `fill.popover`, `fill.tip`, `fill.pane_held`, `fill.paper`, `fill.hover`, `fill.hover_quiet`, `fill.press`, `fill.soft`, `fill.composer`, `fill.composer_pane`, `fill.chip_quiet`, `fill.stat`, `fill.detail`, `fill.file_head`, `fill.well`, `fill.fix`, `fill.step`, `fill.note`, `fill.console`, `dot.hollow`, `dot.uncovered`, `fill.handle` |
| Lines | `line.bar`, `line.divider`, `line.card`, `line.soft`, `line.menu`, `line.swatch`, `line.quiet`, `line.branch`, `line.tree`, `line.picked`, `line.drop_target`, `line.guide` |
| Shadows | `ring.menu`, `ring.card`, `ring.swatch`, `shadow.menu`, `shadow.held`, `shadow.near`, `shadow.tip`, `shadow.button` |
| Controls | `toggle.knob`, `toggle.off`, `segment.on`, `tab.bar`, `tab.on`, `tab.away`, `slider.track`, `meter.track`, `bar.track`, `bar.quiet`, `tag.fill`, `badge.fill` |
| Code | `code.breakpoint`, `code.value`, `code.match`, `code.inlay` |
| Wash | `wash.base`, `wash.from`, `wash.middle`, `wash.to`, `wash.blue`, `wash.teal`, `wash.violet`, `wash.wine`, `wash.edge`, `wash.glow` (strength: the number `wash`) |
| Terminal | `terminal.background`, `terminal.caret_ink` |

## Tests

`tools/harness/README.md` says how a task proves itself with no person watching; this is the short form.

- Unit tests sit in a `#[cfg(test)] mod tests` at the foot of the file they test, or in `<module>/tests.rs` when long. Tests of a crate's public face sit in `crates/<crate>/tests/<topic>.rs`; shared helpers in `crates/<crate>/tests/support/` (a folder with code also has a `main.rs`, so cargo runs its own tests as the target `support`).
- A test returns `Result<(), Box<dyn Error>>`, defends one thing a user would notice, and is named as a sentence: `a_feed_folds_into_its_table`.
- See it fail first, for the reason it exists; if the code came first, break the code and watch the test fail, then restore it. Say what you saw in your report.
- A pane test that reads the tab bar draws `yd_ui::grid_view::view::alone(win, &hub, cx, kind)` over a hub whose `defs` hold the pane's `DEF` and whose `stage` is `Stage::Pane(kind)`. A test that needs the wire or the sources to hear of the shown thread sets `runtime.poll = Some(root::poll)`; one that presses a shortcut sets `runtime.on_key = Some(root::on_key)`. A test that types into a field builds its `PaneCx` with `edits: Some(&win.edits)`: with `None` a tap does not hold the focus.
- **Drawn things** are tested on Rune's in-process `Runtime` (no GPU) through the tree assertions in `crates/ui/tests/support/tree.rs` (`mod support;`, `use support::tree::{..}`; app and panes include it by path): `by_key`, `by_text`, `rect`, `row_text` (the words inside a keyed element, left to right), `assert_row`, `assert_size(tree, key, (w, h), tolerance)`, `assert_golden(tree, "tests/golden/x.txt")`. A golden is one text line per element and is rewritten only with `YD_BLESS=1`; on a difference the fresh copy lands under `CARGO_TARGET_TMPDIR` and the error names both. In `yi-desktop`, `support::scene::elements("daemons/sample")` draws a registered scene with no GPU.
- **Wire code** is tested by replay: `Replay::fixture("approval-allow")?` in `crates/core/tests/support/replay.rs` plays `fixtures/wire/<name>.jsonl` as the server, over a real socket or a `UnixStream::pair`, and checks every frame the client sends (a request by method, a reply to a server ask by id and exact result). A stray, silent, garbled or extra frame is a `Mismatch` naming the recording line. A shape no recording holds is serialised from `yi-types`' own structs.
- **Scenes**: `just scenes` (or `just scenes-only <prefix>`) renders every registered scene to `target/scenes/<pane>/<state>.png` and `.tree.json` and runs parity after each `<slug>/mockup`. **Journeys**: `just journey [name]` runs the built app beside a private `yi serve --model faux/faux-1`: `starts`, then the daily journeys of `tests/journeys/daily` (threads, prompt, restart, files, approve, deny, trace), which type every prompt into the app's own composer. It is not in the task gate.
- Expected values come from the record or a fixture, never from what the code printed.
- In a lane, `cargo test -p <crate>` can build against a stale `yd-core` and fail to compile for a name that exists. Run one test with `cargo test --workspace <name>`, which is what the gate builds.
- An effect a view asks for on layout (see "Errors and unbuilt code") can land a frame after the test's sink is installed: assert that the ask you mean is among those sent, not that it is the only one.
- A test that reads or writes one of the user's files gives the window a folder of its own under the lane's `target/` with `start::files(hub, Files::under(folder))`; `start::quit` waits for the writes. Only the native app reads the real config folder.
- `Table::version()` is a digest of every event, so it proves nothing was sent: apply the events you expect to a copy and compare versions. Reading the status cannot, because one drain folds several events.
- No test starts, stops or changes a real container, service, machine or browser. A source is driven through its injected runner (`docker::serve`, `launchd::serve`, `browser::serve` with a stand-in `Page`) with recorded output, and the test asserts on the exact arguments; an app test hands the asks its sink caught to that loop (`crates/app/src/shell/view/tests/outside.rs`).
- A test that breaks the code to see itself fail restores the file and then touches it: a restored file older than the broken build is not built again, and every later run tests the broken code.

## Errors and unbuilt code

- A crate boundary returns a typed error: a `thiserror` enum in the module that fails, one variant per reaction a caller can have, carrying the offending value and the bound (never unbounded text off the wire, never a secret).
- An enum read from outside the app carries `Other(String)` so a newer peer cannot break it. Unknown fields survive where the data is written back.
- No `unwrap`, `expect`, `panic!`, `todo!`, `unreachable!`, indexing that can miss, or `&text[a..b]`. Use `get`, `?`, `checked_` and `saturating_` arithmetic, `str::get`, `strip_prefix`, `split_once`.
- Unbuilt code compiles and says so: a view calls `placeholder(pane, kind, "<task id>")`; a source sends `FeedStatus::Error("… is not built")`; a fallible function returns its module's `NotBuilt` variant; a step returns no effects. When you build it, remove the `NotBuilt` variant.
- `RefCell` is borrowed with `try_borrow` and `try_borrow_mut`; a failed borrow returns early.
- A text field is drawn from the `TextEdit` its `on_edit` returns: the window's own, in `Win.edits`, reached in a view through `PaneCx.edits`. A copy kept in the hub is only what travels with the pane.
- A field's place in `Win.edits` is its pane and a small name: `yd_ui::win::field(pane, "filter")`, with the name a `const` beside the field (`draft` for the composer, `filter`, `search`, `text`, `row`, `value`). A field of no pane is `(None, name)`, as the command box's. Never make up a `PaneId` to hold a second field.
- A feed row upsert sets the table's status back to live, so a source that wants an error to show sends the status after its rows, and again after any later publish that sent rows.
- A source that reads on a timer does so only while `SourceCmd::Seen(true)` holds; `seen::POLLED` in the app lists its feed beside the pane kind that shows it. It still reads on `Refresh` and after a command.
- A pane that lists only some rows of its feed, or folds some into others, builds its table with `Table::kept` each view run (small feeds only; it leaves the list's fast path).
- A slot of the code surface is laid out as wide as the file's longest line, not the pane: hold its content to `cx.layout_size("<pane key>.code")`.
- A view cannot send. What it must ask once (a file's text, a language server told of an open file) is a zero-size keyed element with `on_layout_tx`: it fires when the element is first laid out and not again while it stays, so key it by what the ask is about (`debugger::source::ask`, the code viewer's `told`).
- A comment is at most two lines and names an incident or an invariant the types cannot hold. Every file starts with a one-line `//!` saying what it owns.

## Addresses

`yi://<kind>/<stable id>[#part]`

- `kind` is a `Kind` slug (`yd_core::kind`): `lines`, `file`, `thread`, `agent`, `container`, … `tools` and `skills` are Yi's and are refused.
- `stable id` is the record's own id: a thread id, a row id, a repository path. It may contain `/`. It is never a title, a label or a position.
- `part` narrows the thing: `L13`, `L13-L20`, a selector, a frame time.
- `%`, `#`, spaces, control and direction characters are written `%XX`; everything else is literal. An address is at most 2,048 bytes.
- Build one with `Address::new(kind, id)?.part(..)`; read one with `text.parse::<Address>()`. A row's address is `Address::new(desc.kind, row.id.as_str())`.

## Commands

- An id is `<owner>.<verb>` in lower case with `-` between words: `layout.flip-chat`, `container.restart`, `row.open`. Declare it as a `pub const` beside its `Command` row.
- A title is what the menu says, in title case. A key is written as Rune parses it: `cmd+k`, `alt+cmd+1`, `cmd+shift+[`.
- `kinds` lists what the command can be aimed at; empty means the app. The right-click menu and a row's detail buttons are built from it.
- A key is shown through `yd_core::command::table::keycap`, the one keycap function, in the mockup's order: `cmd+shift+c` is `⌘⇧C`.
- Run a command with `yd_ui::menu::command_box::run`; do not pick its route by hand unless the view owns it.
- A row never says a button is unavailable: the list says so when the press comes to nothing. A pane's button that needs more than the model is carried by the pane's `carry`, which finds what it acts on from the target's address (an agent's id, a step's id, a line's id) in the thread the window shows.
- A feed or a command id that two crates name is a constant in `yd-core`, imported by both. What a pane asks of a source with arguments is a typed `SourceCmd` in `Msg::Source`, never a text the source parses.

## Commits and reports

- One commit per task on your lane branch. The subject is one plain imperative sentence, at most 72 characters, first word capitalised, no trailing period, no `word:` prefix, no trailers. A body only when the why is not obvious.
- Stage files by name. Never `git add -A`. Never push.
- `just check` inside `desktop/` must exit 0 before you commit; judge it by its exit code.
- Report what you saw fail first, every scene you rendered, and anything you learned about Rune or Yi that is not in `docs/rune-notes.md`.
