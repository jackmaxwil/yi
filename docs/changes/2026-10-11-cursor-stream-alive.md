---
issue: Closes #1226, Closes #1229
raise: crate ai +110, comments +4
---
A Cursor turn keeps sending heartbeats while a tool or a permission prompt runs, so the server no longer sees a silent stream for the length of a long command (Closes #1226). Connecting gives up after 30 seconds (`cursor connect timed out after 30s`), and a host that accepts the connection but never answers, which includes a TLS handshake that stalls, gives up after two minutes (`cursor sent no response within 120s`). An abort during either ends the turn at once. A heartbeat that fails during a tool run is dropped rather than cancelling the tool it was covering. Prose that follows a tool call opens a text block of its own instead of joining the text before the call (Closes #1229).
