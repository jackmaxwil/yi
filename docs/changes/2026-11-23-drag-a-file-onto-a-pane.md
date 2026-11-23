---
issue: Refs #2308
cut: none desktop/crates/core/src/grid/swap.rs
cut: none desktop/crates/panes/src/files/pane.rs
---
In Yi Desktop, drag a file out of the Files pane and let it go on another pane: on the middle it opens as a tab there, on an edge it takes that half, and on the window's edge it takes a column or a row of its own. The file's name follows the pointer and the place it would land is marked before you let go. Escape, or letting go over the Files pane itself, opens nothing.
