# Tracking

The forge is the only register of planned work (D106). Issues, milestones and the project board
on `apex/yi` hold it; nothing in the tree does. `docs/archive/todos-2026-09-02.md` is the frozen
predecessor and is not authored — a row edited there changes nothing.

- An issue number is the identity of the work. Not a file row, not a plan heading, not a commit
  subject: `#N`. Ids that predate the freeze (`A8`, `Q6`, `S3`) survive as the issue's title
  prefix, so `A8 OSC 8 hyperlinks` is findable by either name, and `#N` is what a change cites.
- A feature PR names its issue in the body: `Closes #N` when the merge finishes the work,
  `Refs #N` when it is one part of it. The merge closes the issue itself — nobody marks it
  closed by hand, and a PR that finishes work without `Closes #N` leaves an issue open that
  nothing will ever close.
- Sizes are labels, not prose: `size:S` (≤ 1 day), `size:M` (≤ 3 days), `size:L` (larger).
  Exactly one is mandatory — zero is unsized, two is unsized twice. At least one `area:*` label
  and a milestone are mandatory beside it, because a milestone is what a date is computed over.
- A milestone's due date is arithmetic, not an intention: the weekly job divides the milestone's
  open size-days by the measured throughput and writes the date. A person never types one. A
  date that was typed is a promise; a date that was divided is a measurement, and the difference
  is the whole reason the register moved off a file.
- The board's five columns each mean something checkable, and each is a query rather than a
  mood. **Backlog**: open, no assignee. **Next**: in the current milestone. **In progress**:
  assigned, with a branch. **Blocked**: carries `status:blocked` and the issue says by what.
  **Done**: closed. Done is reached by merging a PR that says `Closes #N`, never by dragging a
  card — a card dragged to Done over an open issue is a lie the weekly job will move back.
- Nothing in the tree is a TODO list. No `docs/TODOS.md`, no roadmap section in a README, no
  `## Open work` heading in a plan. A `// TODO` beside code that names its issue (`// TODO(#41)`)
  is a pointer to the register and is fine; one that names nothing is unowned work and is not.
- An offline session builds and gates; it does not plan. `just check` is green with the whole
  register unreachable, by design (040) — no gate here asks the forge a question. Enforcement
  of any of the above lives in CI, where the forge is reachable and the answer is not guessed.
