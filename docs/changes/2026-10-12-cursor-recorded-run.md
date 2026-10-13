---
issue: Closes #1228
raise: tests +63
---
The Cursor decoder is now checked against a recorded Run from a live `composer-2.5-fast` turn that called the shell once (Closes #1228). The recording showed the server asks for a shell through the streaming-shell exec kind, sends one exec at a time, and waits for its reply before streaming the answer. Stored-blob payloads in the recording, which echo Cursor's own prompt and the account's personal rules, are overwritten at equal length so every frame keeps its real field layout.
