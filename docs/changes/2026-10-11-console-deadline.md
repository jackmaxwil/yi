---
issue: Closes #1256
raise: crate console +6, crate cli +1, tests +27, comments +2
---
`yi console --headless --deadline <secs>` now sets the drive script's wall clock, as `yi tui --headless --deadline` already did. The console ignored the flag and stopped every script at 60 s, so a paced console scene could not outlast it. The default stays 60 s, the same one-day cap applies, and the timeout error names the deadline that tripped (`drive script timed out (N s wall)`).
