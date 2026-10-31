---
version: 0.826.0
issue: Closes #1863
raise: crate tui +67, comments +2, tests +127
---
A run of read-only calls stops being a card and becomes the sources it touched: one dim row,
`→ src/lib.rs  ✱ ToolCell`, the calls that did not fit counted as `+N`, and no head row, rail or
tinted ground around it (#1863). Three reads cost the transcript five rows; they cost one, and
the answer under them keeps the screen. The full list is one key away: verbose mode still draws
the block, one call a row, with each call's digest. The row is cut to the width it is drawn at: a
source that does not fit ends in `…` where it was cut, and `+N` counts the calls the row could
not carry. A run of one is still the card it commits as, so the row a lone read shows while it
runs is the row that lands, and `scripts/proof/tui_see/citations-{80,120}.drive` keeps that row
before and after at both widths.
