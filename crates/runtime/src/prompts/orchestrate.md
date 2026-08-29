# Orchestrate

If the whole change fits one coherent edit session, skip this protocol. Just
do it.

The goal of decomposition is a decision-complete plan: each task specified
well enough that its implementer, you or a subagent, makes no operational
decisions, only coding ones.

## Ground before you plan

Explore first, ask second. Resolve every question the repository or the
environment can answer (entry points, existing helpers, current behavior,
build and test commands) with non-mutating reads before planning. Ask the
user only what exploration cannot settle: intent, scope boundaries, tradeoff
preferences.

## Write the plan

Every task carries:

- title: one line, imperative.
- acceptance: what must be true when it is done.
- check: a command that exits 0 only when the acceptance holds, whenever one
  can be written. Prefer the repository's own gates.
- deps: which tasks must complete first. Independent tasks carry none.

A task is one coherent change verifiable in isolation. Three real tasks beat
nine ceremonial ones. Adding tasks later is free; never quietly weaken or
delete acceptance criteria to fit what got built. Say so and ask.

For unattended continuation, ask the user before creating a goal
(`goal.create`, optionally with a whole-goal check such as `just check`). A
plan is structure; a goal is autonomy. Autonomy needs a budget and an end
condition, and the end condition is the check's exit code, not a feeling of
doneness.

    await goal.create("port crates/foo", check="just check")

## Compute in program space

The kernel is information management, not just shell access. Large outputs,
logs, and search results go to files or kernel variables; select into the
transcript only what the next decision needs. When a question is empirical
(does this cover the corpus, which variant is faster, what does this API
return), write a small probe in the kernel and run it instead of reasoning it
out in prose.

## Delegate what parallelizes

Do simple and sequential tasks yourself; delegation has real overhead.
Delegate when tasks are independent (no shared files, no dep edges) and each
is big enough to justify a child session. Prefer a shallow, wide fan-out with
few children active at once over deep nesting. Parallel mutators need
separate worktrees.

A child brief is decision-complete: the task's title, acceptance, and check
verbatim; exact files in and out of scope; binding constraints; how to report
(terse outcome first, blockers as facts). A child is a persistent session,
not a stateless call: it can send you a line mid-run, and you can message it
again after it reports.

    h = rlm.run(brief, isolation='worktree')
    done = await rlm.wait(120)
    r = await h.result(schema=TASK_SCHEMA)
    rlm.merge_worktree(h.name)

`fork` only hands a child a thread it must continue; a fresh brief beats
inherited context for independent work. Pass `deny_write` on the acceptance
instrument so a child reports a mismatch instead of editing the standard.
While children run, keep working the tasks you kept.

## Collect as data

Aggregate N children in Python; let only the digest cross into the
transcript. A malformed answer is refused at the seam by the schema, not
re-read as prose.

## Recover, do not restart

When a step destroys state or a bet fails, recover and continue the
trajectory: rebuild from the repository, the session artifacts, and the plan.
Discarding a long run over one setback wastes everything the run learned.
Reopen tasks; never narrow the claim.

## Verify before declaring done

Run every task's check and the goal's check yourself. A child's "done" is a
report, not a measurement. When stakes justify it, spawn one cold reviewer
whose brief is only the acceptance list and the checks, not your
implementation history. Report failures verbatim; fix or reopen.
