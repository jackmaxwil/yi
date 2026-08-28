---
name: review
description: >
  Verify finished work against its acceptance criteria with a cold-context
  reviewer. Use before declaring a goal or large task complete, when the
  user asks for a completion review, or when the orchestrate skill's final
  verification step calls for a fresh look. Not a code-style review — this
  checks that the claimed end state is actually true.
---

# Review

Verification by the mind that built the thing inherits its blind spots: the
checks were made inside the scope of the work that produced them. A
reviewer with cold context and only the standard finds what the implementer
cannot.

## Brief a cold reviewer

Spawn a subagent whose prompt contains **only the standard, never your
implementation story**:

- the plan's tasks with their acceptance criteria, verbatim (`plan.get`);
- each task's `check` command and the goal's `check`;
- the original user requirements that matter, verbatim;
- how to report (see below).

Do not include what you did, how you did it, what was hard, or what you
believe works — the reviewer must derive expectations from the standard
and measure the workspace against them.

## What the reviewer does

For every acceptance item: identify the evidence that would prove it, then
inspect the actual current state — run the checks, run the tests, read the
files, execute the binary. Judgments, in order of strength:

1. **proven** — a command exited 0 or the artifact demonstrably matches.
2. **contradicted** — evidence shows the claim false (quote it verbatim).
3. **unverifiable** — the acceptance is too vague to measure; say what
   evidence would be needed.

Match verification scope to claim scope: a narrow check never supports a
broad claim. Never touch harness or verifier paths that belong to an
evaluation — verify with the task's own gates only.

## Report

One line per acceptance item: `task-id · verdict · decisive evidence
(command + exit, or file:line)`. Then a coverage line: what was not
checked and why. Contradicted and unverifiable items reopen tasks
(`plan.edit reopen`, `plan.update ... blocked`) — they never narrow the
claim.
