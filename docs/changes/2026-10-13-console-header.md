---
issue: Closes #1377
raise: tests +89, comments +2, crate console +68
growth: +68 the header row itself, its four buttons and their hit table; folding the conditional tab bar into the row deleted `render_tab_bar` and its extra row, and the chord table gains two entries only because the buttons name them
---
The console has a header row at the top of its main area (Closes #1377), always present. The workspace tabs, which used to take a row of their own only when there was more than one, sit at its left; four buttons sit at its right, copied from Claude Code: terminal `>_`, diff `±`, browser `◉` and files `▤`. They hover like every other control and click through the same hit table. Diff opens the review pane, files opens the `e ` file picker, and terminal and browser say which pane lands them (#1378, #1379) until those panes exist, so the branches that build them only replace one arm. Each has a palette entry and a key: terminal is ⌘Y or ⌥y, browser is ⌘⇧B or ⌥w, files is ⌘O or ⌘⇧E; the tape pane gives up ⌘Y and ⌥y and is reached from the palette. Every row below the header sits one row lower.
