---
version: 0.1051.0
issue: Closes #2207
cut: none crates/console/src/render.rs
cut: none crates/console/src/settings.rs
raise: crate console +136, tests +103
---
The console draws a focused pane's border in the normal panel colour, as it does for every other pane. The session colour stays on the title chip and the scrollbar thumb.

The settings pane marks the chosen row with a full-width highlighted bar. A value still at its default is dim, a changed value is teal and the one being edited is bold. A rule sits under the tab bar. Space changes the chosen setting like Enter, `u` unsets a toggle or choice row, and the hint row names the keys the chosen row takes.
