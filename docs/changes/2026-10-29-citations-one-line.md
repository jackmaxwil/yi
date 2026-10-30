---
issue: Closes #1863
raise: crate tui +77, comments +1, tests +72
---
A run of read-only calls stops being a card and becomes the sources it touched: one dim row,
`→ src/lib.rs  ✱ ToolCell`, the calls that did not fit counted as `+N`, and no head row, rail or
tinted ground around it (#1863). Three reads cost the transcript five rows; they cost one, and
the answer under them keeps the screen. The full list is one key away: verbose mode still draws
the block, one call a row, with each call's digest. The row is cut to the width it is drawn at,
and the count names what the cut hid rather than dropping it silently.
