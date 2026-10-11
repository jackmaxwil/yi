---
issue: Closes #1249
raise: tests +17
---
The console editor's find, undo and redo move to ⌘ chords Ghostty delivers (Closes #1249). Ghostty 1.3.1 binds super+f, super+z and super+shift+z to its own search, undo and redo by default, so ⌘F, ⌘Z and ⌘⇧Z never reached Yi. Find is now ⌘L, undo ⌘U and redo ⌘⇧U; Ghostty neither binds those nor holds them in its menu. ctrl+z and ctrl+shift+z still undo and redo inside the editor.
