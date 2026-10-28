---
issue: Closes #1710
raise: crate tui +7, tests +237
---
The console chat composer stays pinned immediately above the hint bar in empty, new, and resumed sessions. Transcript rows and the empty-state mark remain top-aligned, while drafts, search titles, and popups grow upward into the space between the transcript and composer instead of moving the hint bar.

A first prompt still rolls into the transcript one row at a time while the composer stays fixed. During the console title animation, the small mark lands on the orb’s top-anchored cells and remains hidden until the big title leaves.
