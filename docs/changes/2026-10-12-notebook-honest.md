---
issue: Closes #1355
growth: +438 about 270 lines are the reducer's tests, kept beside it so the reducer stays private (review round 1 refused a public test-only API); deleting the v toggle and the row cache was weighed and refused, because every cap's notice names v as its remedy and the cache is what keeps a full view from rewrapping every frame; the notebook's char-chunk wrapper, the typing box and its two request kinds were deleted
decision: The console notebook pane drops its typing box, so its cells come only from the agent's kernel calls | the 3-row box hid the newest three rows of output, and the owner never types Python there | restore the notebook input and its shift-enter route; `_yi/kernel_execute` stays served for the Python client
raise: crate console +436, crate tui +2, comments +5
---
The console notebook now shows every kernel cell honestly (Closes #1355).

Each cut it makes is a `[…]` row that gives kept of total, the cap and the remedy. The caps are 24 code lines, 40 stdout or stderr rows, 20 result rows, the last 12 traceback rows and 50 cells. Pressing `v` on the pane shows every row. Before this, all five caps cut silently.

- **Cell numbers:** a cell keeps the number it arrived with. Before, the number was the cell's position, so every survivor was renumbered once the oldest cell dropped.
- **Code gutter:** code rows are numbered instead of sitting behind a `│` gutter. That glyph is also in the `cargo tree` box-drawing a recorded cell parsed.
- **Wrapping code:** a wrapped code row hangs under `↪`, four columns past its own indentation, so a continued `if` no longer reads as dedented code.
- **Wrapping output:** stdout and results wrap between words through the shared wrapper rather than every N characters.
- **Long tokens:** a token wider than the row now starts beside its line number instead of leaving the number alone on a blank row.
- **Traceback and stderr:** an error shows its traceback's tail, where the fault is named, rather than `evalue` alone. stderr is now shown at all.
- **Cell header:** the header carries the cell's duration.
- **Typing box removed:** the box that covered the newest three rows is gone.
- **Pane titles:** titles use the pane's width instead of 11 or 14 characters.
