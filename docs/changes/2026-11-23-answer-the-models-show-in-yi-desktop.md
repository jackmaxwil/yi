---
version: 0.1109.0
issue: Refs #2309
cut: none desktop/crates/core/src/wire/frame.rs
cut: none desktop/crates/panes/src/chat/tools.rs
cut: none crates/acp/src/permission.rs
cut: none crates/console/src/app/editor.rs
cut: none desktop/crates/core/src/command/by_kind.rs
---
In Yi Desktop, the model can now point at a file. The file opens as a tab where code goes; it comes to the front only if you have left the app alone for ten seconds, and otherwise waits as a tab with a dot, and nothing ever takes your keys. The step in chat says "Showed" and has a "Show me" link that brings the file forward at its line.
