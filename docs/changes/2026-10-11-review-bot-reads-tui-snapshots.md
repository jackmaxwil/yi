---
issue: Closes #1210
---
The review bot now reads the TUI snapshot comment. When the `tui` job's comment was made for the head a round reads, every lens gets its cell diff as text beside the diff, without the picture links, capped at 20,000 characters with the cut named. A comment made for an older head is withheld and the lens is told so. The comment carries the head it was made for in a hidden note under its first line, so the bot can tell.
