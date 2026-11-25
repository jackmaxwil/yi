---
issue: Closes #2383
decision: in the console a press on a tool card or a folded thought opens that one cell whole where it stands and the next press folds it, and no pane opens beside the chat; a shell command's head is kept whole and cut only past its card's rows, a long result row takes the rows its card has left, and a card keeps the first 80 and last 40 lines of its result (revises the press-opens-a-pane clause of the one row budget) | the owner: "tool calls are being cut off prematurely. i think it works by a hard char limit. i think it should be more dynamic based on available tui screen real estate. i should be able to expand tool calls inline with a mouse click, both the tool call invocation itself and the result", and "opening thinking opens on the right side in its own panel. i dont want it in its own panel. should be expanded inline" | reversible by restoring `open_thought` and the 120-character cut in `arg_summary`'s callers
ledger~: TUI | the console opens a folded thought beside the chat on a press | in the console a press on a tool card or a folded thought opens that one cell whole where it stands and the next press folds it, a shell command's head is whole up to its card's rows, and a long result row takes the rows its card has left
ledger~: TUI | drive::pressing_a_folded_thought_opens_it_beside_the_chat | drive::pressing_a_folded_thought_opens_it_in_place
raise: tests +64, crate tui +12
cut: none crates/tui/src/cell.rs
cut: none crates/tui/tests/cards.rs
cut: none crates/tui/tests/toolcells.rs
cut: crates/console/src/app/menu.rs | pub(super) fn open_thought(&mut self, pane: PaneId, cell: usize) { | the owner does not want a thought in its own pane; a press opens it in place
cut: crates/tui/src/app/pane.rs | pub fn folded_thoughts(&self) -> Vec<(Rect, usize)> { | every chat row is pressable now, so thoughts need no hit table of their own
cut: crates/console/src/hit.rs | Thought(PaneId, usize), | the same: a press on any transcript row toggles its cell
---
In the console a click on a tool card or a folded thought opens that one cell whole where it stands, and a second click folds it. The pane that a press on a thought used to open beside the chat is gone. A shell command's head is no longer its first line cut at 120 characters: the whole command is kept and shown up to the rows its card has, then cut with `…` until the card is opened. A long result row takes the rows its card has left, never fewer than two, where it was held to two. The chat keeps the first 80 and last 40 lines of a result where it kept 12 and 6, so an opened card, and the verbose mode, show what the model read. The solo chat has no mouse; ctrl+o still opens everything there. Yi Desktop follows in #2384.
