---
issue: Closes #1294
raise: crate tui +5, comments +2, tests +53
---
A console pane's first prompt now rolls in a row a frame like everything after it. The pane's row pacer pins its window to the transcript cell at its top, and a fresh pane had no cell yet, so the turn's first prompt landed in one frame and the composer under it jumped with it. While a turn runs and the transcript holds its one first cell, the window now starts pinned to that cell's top. A pane scrolled away and back, or resized, during that first turn settles where it is rather than rolling the prompt in again.
