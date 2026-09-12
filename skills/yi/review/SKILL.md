---
name: review
description: >
  Verify finished work against its acceptance criteria with a cold-context
  reviewer. Use before declaring a goal or large task complete, when the
  user asks for a completion review, or when the orchestrate skill's final
  verification step calls for a fresh look. Not a code-style review — this
  checks that the claimed end state is actually true.
trigger: review the work, verify the goal, acceptance criteria, cold review
scope: text
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

For every new test: name the fixture's shape and where it came from; a
hand-typed fixture that is not the smallest field-failing input is
`unverifiable`, and say what artifact would make it `proven`.

For every asserted user-facing sentence: read it on the rendered surface
beside its neighbours; a contradiction with the next row is `contradicted`,
quoted verbatim.

Match verification scope to claim scope: a narrow check never supports a
broad claim. Never touch harness or verifier paths that belong to an
evaluation — verify with the task's own gates only.

Spawn implementer children with `deny_write` over the acceptance instrument so
the standard cannot drift while the work is judged against it; the reviewer
itself keeps read access unless the instrument is sampled, where `deny_read`
hides it from the implementer as well.

## Report

One line per acceptance item: `task-id · verdict · decisive evidence
(command + exit, or file:line)`. Then a coverage line: what was not
checked and why. Contradicted and unverifiable items reopen tasks
(`plan.edit reopen`, `plan.update ... blocked`) — they never narrow the
claim.

## The todo audit

Before the verdict, read the session's todo list (`yi todo` prints the
newest, or `view` from inside the session). For every item marked done,
find the evidence it quotes and check it against the tree: a `done` with
no evidence, or with evidence the tree does not bear out, is a finding
before any other. An item still running or pending at the end of a turn
that claimed completion is the first line of the report. The assessment
rules apply to every number you quote: the command, the scope, and the
matches read.
