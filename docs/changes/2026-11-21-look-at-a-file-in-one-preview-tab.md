---
issue: Refs #2306
cut: none desktop/crates/panes/src/code_view/pane.rs
cut: desktop/crates/panes/src/design/pane/tests.rs | fn tap(rt: &mut Runtime<Win>, key: &str) -> Check { | the design and code viewer tests share one tap helper in test_support.rs
---
In Yi Desktop one press on a file in the Files pane now looks at it in a preview tab, drawn with an italic title, and the next file looked at takes that tab, so reading through a folder no longer leaves a tab a file. A double press on the row, or a press on the preview tab while it shows, keeps the file as a tab of its own. Files opened from chat, Changes, a finding or a command are kept as before.
