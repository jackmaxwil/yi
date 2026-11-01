---
version: 0.848.0
issue: Closes #1949
growth: +398 reuses the fence renderer, the hit table and the terminal pane, so no new pane or protocol; deleting the chip leaves the offer to a copy-paste, and about 40 lines are the split `markdown.rs`, `app.rs` and `render.rs` needed to stay under their file ceilings
raise: comments +21
raise: crate console +215
raise: crate tui +180
raise: tests +419
---
A shell fence in a finished answer of a console chat pane now ends its header row in a `▶ run` chip, the way Claude Code offers a command to run. A press on the chip types the block into the terminal pane the person is looking at, or the one that tab's chips fed last, or opens one beside the chat in the session's own working directory, and presses enter, so the person sees the command run in their own shell and the agent is told nothing. Fences labelled `bash`, `sh`, `shell` and `zsh` get a chip; any other fence, and a paragraph that spells the chip, do not. `Theme.run` says a surface can press a chip: only the console's chat panes set it, so the solo transcript, the markdown and diff viewers and a child's card draw none. An answer is rendered whole once its message is sealed, because the growing render cannot tell a closed fence from an open one; a chip appears when the message ends, never on a fence still arriving.

`markdown.rs` gave its `Settled` streaming cursor to `markdown/settled.rs`, its chip helpers live in `markdown/run.rs`, `app.rs` gave `pane_row` and the new chip lookup to `app/pane.rs`, and the console's `Hit` table moved to `hit.rs`, each a split at a seam so no file past its ceiling grew.
