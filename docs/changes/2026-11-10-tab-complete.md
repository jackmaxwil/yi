---
issue: Refs #2142
cut: none crates/tui/tests/input.rs
shares: #2137 the imported draft this branch replaces
shares: #2138 sibling import touching the same composer and console test files, being fixed in its own lane
shares: #2136 imported draft being split, touches render.rs
shares: #2135 imported draft touching render.rs
shares: #2134 imported draft touching the console drive tests
ledger+: TUI | Tab fills the highlighted slash command into the bordered box with a trailing space, and Enter in the `/` list still runs the highlighted match
---

In the `/` command list the typed name lives in the bordered text box, with the matches above it. Tab writes the highlighted command there and leaves a trailing space for arguments. Enter runs the highlighted match, so `/mo` Enter runs `/model`. Esc and Ctrl-C close the list and empty the box, as before. A draft that is `/` plus a command prefix does the same on Tab: one match is filled in place, several open the list.

The paste test that expected the typed text inside the list is rewritten for the new model: a paste into an open list now fills the box.
