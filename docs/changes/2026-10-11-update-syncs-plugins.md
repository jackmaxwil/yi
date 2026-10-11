---
issue: Closes #1192
raise: crate cli +46, comments +2
---
`yi update` now finishes by running the installed binary's `yi plugin sync`. A release that pins a newer ripwire used to leave the old one installed, `yi doctor` failing on it, until the user ran the sync by hand; the new binary's plugin table now brings it to the pin as part of the update. A failed sync is the update's exit code, with the plugin's own error above it. With no plugin installed the update ends with `nothing to sync: yi installed no plugin here`.
