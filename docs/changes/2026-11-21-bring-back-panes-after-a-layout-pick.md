---
version: 0.1098.0
issue: Refs #2307
cut: none desktop/crates/core/src/grid/step.rs
cut: none desktop/crates/core/src/grid/layout/store.rs
cut: none desktop/crates/core/src/grid/windows.rs
---
Yi Desktop no longer loses a pane that opened by itself when you pick a layout. Before, Agents, Changes, the Web app or IPython vanished on the pick and stayed gone for the rest of the session; now each comes back as a quiet tab while the session still has something for it. Just chat stays the chat alone.
