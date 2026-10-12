---
issue: Closes #1283
raise: comments +2, crate console +1, crate tui +9, tests +112
---
A control character fed in as a keystroke is dropped before any input sees it (Closes #1283, #1284, #1285, #1286, #1287). A paste fed as keys carried ESC, BEL or DEL into the slash popup query in solo, the console composer behind a slash command, the console palette and find query, the first prompt that names a console tab, and the notebook pane input, and the drive refused the frame. `is_pasted_control` is the one predicate; `SingleKey::from_event` and the console's `handle_event` both consult it, so the solo and console chats and every console overlay share the guard. Also adds the `open_apps` and `pace` fields the console deadline test had missed in `ConsoleOptions`, which left the console test target uncompilable on main.
