---
issue: Refs #2187
---
`just commit` now refuses a commit that leaves a file with five or more lines lost on net and no `cut:` or `keep:` line in the branch's change file, naming the line to write. This is the same check `just push` and `just pr open` already ran, so the refusal comes seconds after the edit instead of after the pre-push lane. `just pr open --help` now shows `--body PATH` and says it takes a file.
