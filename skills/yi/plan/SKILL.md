---
name: plan
description: >
  Write a decision-complete plan on the plan tool when work will be handed
  to children, when tasks carry checks the runtime should run, or when the
  dependency order matters more than the reading order. Use when the user
  says "plan this", "create a plan", "write a plan", or when a request spans
  several subsystems and several constraints. Do NOT use for a three-step
  task; the todo list is the list.
trigger: plan this, create a plan, write a plan, decision-complete
scope: text
---

# plan

`plan` → ground → lift the todos → a check per task → `plan` ops → the user reads the DAG

## Ground before you plan

Resolve every question the repository or the environment can answer with
reads and non-mutating commands: entry points, existing helpers, current
behaviour, build and test commands. Present discoverable facts as
candidates with a recommendation. Ask the user only a preference between
real tradeoffs, as two to four options with a default, through `ask_user`,
and proceed on the default if unanswered, saying so.

## Decision-complete

The implementer, you or a child, makes no operational decisions, only
coding ones. Every task carries:

- title: one line, imperative;
- acceptance: what must be true when it is done;
- check: a command that exits 0 only when the acceptance holds, whenever
  one can be written (prefer the repository's own gates);
- deps: the tasks that must complete first.

Group tasks by behaviour or subsystem, not by file; avoid naming more than
three paths per task. Never invent a schema, precedence rule or wire shape
the request did not establish; say it is open.

## Good and bad

Good:

```
- Parse folded frontmatter (check: cargo nextest run -p yi-runtime -E 'test(a_folded_description)')
- Route project skills under $HOME to the yard (check: … -E 'test(a_project_under_home)')
- Dedupe identical instruction files (check: … -E 'test(identical_instruction_files)')
```

Bad:

```
- Fix skills
- Make catalog better
- Tests
```

## The ops

`plan init` with the goal and the todos; `plan start`, `done`, `fail`,
`block`, `unblock` step one; `plan decompose` opens a sub-plan under a
running todo; `plan supersede` replaces the cut with a reason. A todo moves
pending → running → done in order: `done` on a pending todo and several
`done` at once are refused. The todo tool stays the day-to-day list; a
plan task may mirror a todo, and the plan steps it.

## Delegation

A child brief is the task's title, acceptance and check verbatim, the
files in and out of scope, the binding constraints, and how to report.
Pass `deny_write` on the acceptance instrument so a child reports a
mismatch instead of editing the standard.

    h = await rlm.run(brief, isolation='worktree')
    await rlm.wait(120)
    r = await h.result()

A child's done is a report; run the check yourself before stepping the
task.
