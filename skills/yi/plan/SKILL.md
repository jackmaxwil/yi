---
name: plan
description: >
  Write a decision-complete plan with `yi.Plan` when work will be handed
  to children, when tasks carry typed Contracts the runtime should run, or when
  the dependency order matters more than the reading order. Use when the user
  says "plan this", "create a plan", "write a plan", or when a request spans
  several subsystems and several constraints. Do NOT use for a three-step
  task; the todo list is the list.
trigger: plan this, create a plan, write a plan
---

# plan

`yi.Plan` declares work with typed `Contract`s; `rlm.plan_op` sends operations to the registered runtime `plan.op` route.

## Ground before you plan

Resolve every question the repository or the environment can answer with
reads and non-mutating commands: entry points, existing helpers, current
behaviour, build and test commands. Present discoverable facts as
candidates with a recommendation. Ask the user only a preference between
real tradeoffs, as two to four options with a default, in your reply (`ask_user` in a child or with auto-review on),
and proceed on the default if unanswered, saying so.

## Decision-complete

The implementer, you or a child, makes no operational decisions, only
coding ones. Every task is a todo with these fields, declared with `yi.Plan`
or the registered `plan.op` route:

<!-- yi:schema plan /properties/todos/items -->
- `label` (string, required): the todo's name, imperative, at most 80 chars
- `state` (string, one of `pending`, `running`, `done`, `blocked`, `failed`, `dropped`): the state the row should reach: done runs its contract first; blocked asks (on, note, options); failed records its cause; dropped removes the row; pending or running reopens a failed or blocked row. A row the engine cannot move says why in the reply
- `after` (list of string): labels of the todos this one waits on
- `intent` (list of string): user://<n> of each user message it serves; default the latest
- `waived` (list of object): [{address, reason}]: a user message the plan leaves unserved
- `delegation` (object): {spec: {role?, model?, effort?, isolation?}, accept: {command} | {stated}, context?: [url], output?: {schema: url}}
- `contract` (object): what must hold when the todo is done; done runs its items and passes at the threshold
- `todos` (list of object): the sub-steps this row splits into, rows of its own sub-plan; the row runs while they do
- `on` (object): blocked: {"child": agent} | {"user": null} | {"external": {"probe": command}} | {"channel": {"address": "clock://at <ISO time>" or an exec://, file:// or channel:// address as in todo, "filter"?}}, unblocked by the first match; default the user
- `note` (string): blocked: what would unblock it
- `options` (list of object): blocked on the user: 3 to 5 answers [{id, label, preview?}] the user picks one of by replying with its number, id or label; a preview is light (a line, a small diagram's source, or an address), at most 2048 bytes
- `cause` (string): failed: what went wrong
<!-- /yi:schema -->

Each todo's acceptance is a typed Contract. Build it with `contract(...)` from
`cmd`, `schema`, or `example`; Python renders any referenced artifacts before
sending the op.

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
- Parse folded frontmatter (accept: `contract(cmd("cargo nextest run -p yi-runtime -E 'test(a_folded_description)'", critical=True))`)
- Route project skills under $HOME to the yard (accept: `contract(cmd("cargo nextest run -p yi-runtime -E 'test(a_project_under_home)'", critical=True))`)
- Dedupe identical instruction files (accept: `contract(cmd("cargo nextest run -p yi-runtime -E 'test(identical_instruction_files)'", critical=True))`)
```

Bad:

```
- Fix skills
- Make catalog better
- Tests
```

## The registered operations

`Plan.create` opens the current `.yi/plans` ledger. `Plan.todo(..., accept=contract(cmd(...)))`
declares a todo with a typed Contract. These calls go through `yi.Plan` to `rlm.plan_op` and
the registered runtime `plan.op` route. `cmd(...)` builds a frozen Contract item, not a separate
check call; todos have no `check` field.

`Plan` has no `split()` or targeted-append method. Use `rlm.plan_op` for both operations. Split
names the parent and gives each child a title, prose acceptance, and optional typed Contract:

```python
from yi import cmd, contract

accept = contract(cmd("pytest -q -k empty_input", critical=True))
wire_contract, blobs = await accept.render("inline", lambda url: rlm.fetch(url, as_text=True))
reply = await rlm.plan_op("split", {
    "label": "parse input",
    "subtasks": [{
        "title": "Handle empty input",
        "acceptance": "The parser accepts and reports empty input.",
        "contract": wire_contract,
    }],
}, artifacts=blobs)
```

Targeted Append uses the same typed Contract wire form and names its target with `for_task`:

```python
reply = await rlm.plan_op("append", {
    "for_task": "parse input",
    "todos": [{"label": "Record empty-input behavior", "contract": wire_contract}],
}, artifacts=blobs)
```

Red 1 permits one more claim. Red 2 blocks the next `done` before a check runs, so change
structure with Split or targeted Append; `Append.for_task` readmits only its named red-2 todo
for one verification. If that verification is red, the task reaches red 3. Split and targeted
Append cannot clear red 3. Stop and ask the user, then wait for a current user answer. Refusals
and abstentions do not count as red verdicts.

`plan.op` returns a reply with `ok`; refusals are data in `reply["refusal"]`. Read the full plan
with `rlm.plan_op("view", {"full": True})`.

## Delegation

A child brief carries the todo's title, prose acceptance and typed Contract, plus the files in
and out of scope, the binding constraints, and how to report.
Pass `deny_write` on the acceptance instrument so a child reports a
mismatch instead of editing the standard.

    h = await rlm.run(brief, role='root', isolation='worktree')
    await rlm.wait(120)
    r = await h.result(timeout=420)

A child's done is a report; the runtime verifies its Contract before the todo reaches `done`.
