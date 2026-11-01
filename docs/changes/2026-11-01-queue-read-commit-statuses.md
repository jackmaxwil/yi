---
issue: Refs #1966
---
`just pr status`, `just pr merge` and the autofix bot now read a head's lanes from its commit statuses instead of the last sixty rows of the task list. That list has no row for a job still waiting on a runner, so a rerun read as its old failure, and sixty rows is an hour or two of forge activity. `just pr rerun` also refuses while a gate lane is pending, because a second reopen cancels the waiting run and writes failure on every context; the bot takes that refusal as waiting and keeps its one rerun on the head.
