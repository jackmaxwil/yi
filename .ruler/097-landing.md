# Landing

A branch is finished when its pull request is merged on the forge, not when its tests pass
here. The steps between the two are where sessions fail, so they are verbs, not habits:
`just ratchet`, `just commit`, `just push`, `just pr open`, `just pr status`, `just pr merge`,
or `just land` to open the PR as a draft the review bot reads; it merges after `just pr ready`
passes on two clean rounds. The yi-forge skill is the procedure; this is the law.

- The size ceilings move with no commit: a raise is a line in the change file (040). Every
  other baseline moves in its own `Ratchet: …` commit, never beside the code commit: a raised
  ceiling moves first (040) — `just commit` refuses a code commit while a ratchet is red — a
  shrunk one moves after. `just ratchet` makes that commit; `just commit` refuses to bury one.
- A subject is at most 72 characters, imperative, no trailing period. `just commit` and
  `just pr open` judge it with the gate's own function before anything is written.
- A push is the pre-push lane. It takes minutes and it is not optional: run `just push` in the
  background with a long timeout and read its exit, never a foreground call that a timeout kills
  half way, which pushes nothing and says nothing.
- A pull request cites an open issue with one `size:` label, an `area:` label and a milestone,
  and the change file it adds cites the same `#N`. `just pr open` runs the `title` job's judge
  locally and refuses before the forge does.
- A green pull request still does not merge once `main` moved: the forge answers "head behind
  base" only to the API. `just pr merge` updates the branch on the forge and retries; a refusal
  is printed verbatim, never retried blind.
- Job logs are not on the API. `just pr status` says which job failed; the log is
  `just ci-log <pr>` in the infra repository or the job page.
- A branch is never rebased after it is pushed. Recreate it if the history must change.
