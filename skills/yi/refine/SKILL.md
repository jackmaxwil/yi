---
name: refine
description: >
  Hill-climb one deliverable against a check: make up to three variants of a
  change, score each with the task's own verifier, keep the best, stop at the
  budget. Use when the user says "refine this", "keep improving until", or
  asks for the best of several attempts. In the Yi repository, improving Yi
  itself is `just refine round`, not this.
trigger: refine this, keep improving
---

# Refine a deliverable

A refine round is a plan whose todos are variants and whose contracts are the
task's verifier. Nothing here is a new tool: it composes `plan`, `scatter` and
the contract `cmd` decider the plan engine already runs.

1. **Name the score before any variant.** The score is a command that exits 0
   on success or prints `{"score": x}` on its last line: the test suite, a
   benchmark script, a linter count. A score you cannot run is not a score; ask
   for one instead of judging prose, which passed 3 of 18 checks it should have
   failed when Yi tried it (D546).
2. **Freeze what the variants may not touch.** The verifier, its fixtures and
   any hidden tests go in the brief's `deny_write`, so no variant can raise its
   own score by editing the grader.
3. **Write the plan.** One todo per variant, each delegated to a worker in its
   own worktree with the score as a critical `cmd` item, plus a final todo that
   re-runs the score on the kept variant. `yi.shapes.scatter` runs those todos as
   one fan-out; `read("yi://skills/plan")` has the op shapes.
4. **Keep the best, by the number.** Accept the variant with the highest score;
   on a tie, the smaller diff. Merge it with `rlm.merge_worktree` and discard the
   rest with `rlm.discard_worktree`.
5. **Stop at the budget.** Give every child a token lease and the plan a
   deadline. Three variants and two rounds are the ceiling; a round that keeps
   nothing ends the loop and says which check held every variant back.

Report the score of the base, of each variant and of the kept one, and the
command that produced them, so the user can run it again.
