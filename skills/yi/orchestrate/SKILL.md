---
name: orchestrate
description: >
  Decompose a large software task into a verified plan and, where it pays,
  parallel subagent work. Use when a task spans several files or subsystems,
  when the user asks for a plan, when independent workstreams could run in
  parallel via rlm subagents, or when a long run keeps growing without a
  written task list. Not for small single-file changes — just do those.
---

# Orchestrate

The goal of decomposition is a **decision-complete** plan: each task is
specified well enough that its implementer — you, or a subagent — makes no
operational decisions, only coding ones.

## 1. Ground before you plan

Explore first, ask second. Resolve every question the repository or the
environment can answer (entry points, existing helpers, current behavior,
build and test commands) with non-mutating reads before planning. Ask the
user only what exploration cannot settle: intent, scope boundaries,
tradeoff preferences.

## 2. Write the plan

Write the task list down before editing. Every task carries:

- **title** — one line, imperative.
- **acceptance** — terse prose criteria: what must be true when it is done.
- **check** — an executable command that exits 0 only when the acceptance
  holds, whenever one can be written (a test filter, a build, a grep that
  must be empty). Prefer the repository's own gates.
- **deps** — which tasks must complete first. Independent tasks carry none.

Sizing: a task should be one coherent change an implementer can verify in
isolation. Do not pad the list — three real tasks beat nine ceremonial ones.
Adding tasks later is free; never quietly weaken or delete acceptance
criteria to fit what got built — say so and ask.

If the work should continue unattended, ask the user before creating a goal
(`goal.create`, optionally with a whole-goal `check` such as `just check`).
Never create a goal uninvited: a plan is structure, a goal is autonomy.

## 3. Decide what to delegate

Do simple and sequential tasks yourself — delegation has real overhead.
Delegate when tasks are independent (no shared files, no dep edges between
them) and each is big enough to justify a child session. Parallel mutators
need separate worktrees; do not fan out file-writers into one tree.

## 4. Brief each subagent

A child prompt is a decision-complete brief:

- the task's title, acceptance, and check, verbatim;
- the exact files and directories in scope, and what is out of scope;
- constraints that bind (style rules, banned dependencies, gates to keep
  green);
- how to report: terse outcome first, blockers as concrete facts, no
  narration.

Spawn with `rlm.run(brief)`; `isolation='worktree'` gives a mutating child
its own checkout, and `rlm.merge_worktree(name)` / `rlm.discard_worktree(name)`
hands it back (a child holding a worktree cannot be reaped until one of them
runs). Use `fork='all'` or `fork=<n>` only to hand a child the thread it must
continue — a fresh brief beats inherited context for independent work.
Pass `deny_write=[<instrument paths>]` to an implementer child when a task's
acceptance instrument lives in the tree: the standard is fixed for the run, and
a child that cannot edit it reports the mismatch instead.
Tell the child how to report mid-run: `await rlm.send('parent', '<line>')`
reaches you without ending its turn, so a blocker arrives when it is found
rather than when the child finishes.

While children run, keep working the tasks you kept.

## 5. Collect in program space, not in the transcript

`await rlm.wait(timeout=…)` blocks until a child reports or finishes and
returns the names that moved — use it instead of polling `list_subagents` in a
loop. Then take each result as data:

    done = await rlm.wait(120)
    results = [await h.result(schema=TASK_SCHEMA) for h in handles]
    blockers = [r["json"] for r in results if r["json"].get("blocked")]

`handle.result(schema=…)` validates host-side and raises on a mismatch, so a
malformed answer is refused at the seam instead of becoming prose you have to
re-read. Filter and aggregate N children in Python; let only the digest cross
into your transcript. Then verify each child's work against its acceptance
yourself — a child's "done" is a report, not a measurement.

## 6. Verify before declaring done

Run every task's check and the goal's check. A fresh look finds what the
implementer cannot: when the stakes justify it, spawn one cold reviewer
subagent whose brief is only the acceptance list and the checks — not your
implementation history — and have it verify each item. Report failures
verbatim; fix or reopen tasks rather than narrowing the claim.
