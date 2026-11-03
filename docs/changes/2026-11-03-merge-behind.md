---
issue: Closes #2007
---
`just pr merge` no longer updates a pull request merely because main moved. With the merge queue on, a behind, conflict-free PR goes to the queue, whose candidate is gated on current main; a real conflict still takes the explicit update path. A queue landing now runs the recorder on its candidate and pushes the candidate and record together, with the record authored by yi-queue so postmerge does not record it again. The queue's baseline merge driver uses a worktree-relative command instead of a path that dangles when the worktree is deleted.
