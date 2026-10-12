---
issue: Closes #1210
---
The review bot now reads the TUI snapshot comment. When the `tui` job's comment was made for the head a round reads, every lens gets its cell diff as text beside the diff, without the picture links, capped at 20,000 characters with the cut named. A round started by the same push waits up to 30 minutes for the `tui` job's comment step to finish. A comment made for an older head is withheld and the lens is told so. The rows are framed as evidence the PR's own binary drew, and a row naming the block's tag is dropped so it cannot close the block. The comment carries the head it was made for in a hidden note under its first line, so the bot can tell.
