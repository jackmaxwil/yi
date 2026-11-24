---
issue: Refs #2308
cut: none desktop/crates/core/src/grid/step/resting.rs
cut: none desktop/crates/ui/src/menu/context.rs
---
Yi Desktop's right-click menu on a pane's bar can now move the pane to another place: "Move to work", "Move to side" or "Move to shelf". It joins that place's tabs in front, or opens the place if nothing is there, and stays of that place. The menu also stays inside the window now: with the rows a bar has, it ran off the bottom of the window for a pane low in it.
