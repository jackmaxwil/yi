---
name: plan
description: >
  Write a decision-complete plan on the plan tool when work will be handed
  to children, when tasks carry checks the runtime should run, or when the
  dependency order matters more than the reading order. Use when the user
  says "plan this", "create a plan", "write a plan", or when a request spans
  several subsystems and several constraints. Do NOT use for a three-step
  task; the todo list is the list.
trigger: plan this, create a plan, write a plan
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
coding ones. Every task is a todo with these fields, rendered from the plan
tool's schema:

<!-- yi:schema plan /properties/todos/items -->
- `label` (string, required): the todo's name, imperative, at most 80 chars
- `after` (list of string): labels of the todos this one waits on
- `intent` (list of string): user://<n> of each user message it serves; default the latest
- `waived` (list of object): [{address, reason}]: a user message the plan leaves unserved
- `delegation` (object): {spec: {role?, model?, effort?, isolation?}, accept: {command} | {stated}, context?: [url], output?: {schema: url}}
- `contract` (object): what must hold when the todo is done; done runs its items and passes at the threshold
<!-- /yi:schema -->

Its acceptance is a contract whose items each carry a `decider`, a command
that exits 0 only when the item holds whenever one can be written (prefer
the repository's own gates):

<!-- yi:schema plan /properties/todos/items/properties/contract/properties/items/items -->
- `id` (string, required): the item's name, unique in the contract
- `critical` (boolean, required): a failed critical item fails the todo, an abstaining one holds it
- `weight` (integer, required): the item's share of the score
- `decider` (object, required): {cmd: "shell command that exits 0 only when the item holds"}, or {cmd: {checker: command, timeout_ms}} (default and ceiling 600000); {schema: {schema: artifact}}; {example: {cases: artifact, runner: artifact, timeout_ms}}; an artifact is {digest, media_type, length}
<!-- /yi:schema -->

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

    h = await rlm.run(brief, role='root', isolation='worktree')
    await rlm.wait(120)
    r = await h.result(timeout=420)

A child's done is a report; run the check yourself before stepping the
task.
