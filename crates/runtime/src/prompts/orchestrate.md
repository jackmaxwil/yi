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

A todo's fields, rendered from the plan tool's schema:

<!-- yi:schema plan /properties/todos/items -->
- `label` (string, required): the todo's name, imperative, at most 80 chars
- `after` (list of string): labels of the todos this one waits on
- `intent` (list of string): user://<n> of each user message it serves; default the latest
- `waived` (list of object): [{address, reason}]: a user message the plan leaves unserved
- `delegation` (object): {spec: {role?, model?, effort?, isolation?}, accept: {command} | {stated}, context?: [url], output?: {schema: url}}
- `contract` (object): what must hold when the todo is done; done runs its items and passes at the threshold
<!-- /yi:schema -->

A contract item's fields:

<!-- yi:schema plan /properties/todos/items/properties/contract/properties/items/items -->
- `id` (string, required): the item's name, unique in the contract
- `critical` (boolean, required): a failed critical item fails the todo, an abstaining one holds it
- `weight` (integer, required): the item's share of the score
- `decider` (object, required): {cmd: "shell command that exits 0 only when the item holds"}, or {cmd: {checker: command, timeout_ms}} (default and ceiling 600000); {schema: {schema: artifact}}; {example: {cases: artifact, runner: artifact, timeout_ms}}; an artifact is {digest, media_type, length}
<!-- /yi:schema -->

The label is a name, not a sentence. Give a contract whenever a command can
judge the work, preferring the repository's own gates; independent todos
carry no `after`.

A checklist you work yourself, with nothing for the engine to run, is one
`op=set` call with a `goal`.

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
Readers first: a fan-out of walled readers over disjoint areas is what a
large task reaches for before any plan exists. Writers when tasks are
independent (no shared files, no dep edges) and each is big enough to
justify a child session; parallel writers need separate worktrees.

A plan you hand out is a `yi` program in the kernel: todos with a
delegate and a contract, run by a shape; `help(yi)` has the rest. The host
admits, spawns, verifies at `done` and merges a worktree; read
`run.outcome`, and keep `budget` under the cell's ten minutes.

A reader brief is one question, the places to look, what not to conclude,
and the findings shape. `scatter` binds each reader to a partition and
hands your `lead` only answers whose quoted lines it found there; open a
cited line yourself before building on it.

    from yi import Plan, Reader, contract, schema, scatter, shapes
    areas = {"auth": "local://crates/auth", "storage": "local://crates/store", "cli": "local://crates/cli"}
    plan = await Plan.create("map the session token")
    for name, root in areas.items():
        ask = "Where is a session token minted, stored and checked? Quote each line; conclude nothing."
        await plan.todo(key=name, delegate=Reader(partition=[root], note=ask),
                        accept=contract(schema(shapes.ANSWER, critical=True)))
    async def lead(answers, number):
        return {"commit": answers} if answers else {"ask": "Quote the line that mints it."}
    await plan.todo(key="lead", run=lead, accept=contract(schema({"type": "object"}, critical=True)))
    run = await plan.run(shape=scatter, budget="8m")

A writer brief is decision-complete: the task's title, acceptance, and
check verbatim; exact files in and out of scope; binding constraints;
blockers reported as facts, not questions. The check is the todo's
contract, so `done` runs it, not the child. A child is a persistent
session, not a stateless call: it can send you a line mid-run, and you can
message it again after it reports.

    from yi import Plan, Writer, contract, cmd, fork_join
    plan = await Plan.create("port foo and bar to the new API")
    for name in ("foo", "bar"):
        brief = f"Port crates/{name} to the new API. Scope: crates/{name}/** only. Read kernel://main/api_notes."
        await plan.todo(key=name, delegate=Writer(accept=contract(cmd(f"cargo test -p {name}", critical=True)),
                                                  deny_write=["docs/"], note=brief))
    run = await plan.run(shape=fork_join, budget="8m")
    print(run.outcome, run.refusals)

A shape starts what is ready up to the host's admission count and settles
children as they land; a second `plan.run` with the same shape attaches to
the first. `fork` only hands a child a thread it must continue; a fresh
brief beats inherited context for independent work. Pass `deny_write` on
the acceptance instrument so a child reports a mismatch instead of editing
the standard. While children run, keep working the tasks you kept.

Context reaches a child four ways: `context_keys` for what you computed
(the brief), `kernel://main/<var>` for what is live, `family://<name>`
for what was `put`, `tree://<name>/<path>` for a file in a worktree.

When a child fails or `rlm.status()` says `stuck`, read its tail
(`history://<name>/tail/20`), fix the brief, and respawn; never repair
inside a child's tree by hand. A cold reviewer is a reader whose brief is
only the acceptance list and the checks. The caps: depth one unless the
config raises it, eight children per parent, sixteen live per family.

## Patterns

A bare `rlm.run` is a question-child: it reads and answers once. A child
that makes one change is `role="worker"`: the project's rules, edit tools,
its partition, no kernel. Every pattern here but map-reduce spawns
`role="root"`, a full child.

- Map-reduce: `rlm.ask` per shard with a `schema`; you reduce the
  answers with pandas in your kernel.
- Hypothesis tournament: N children with `check=`; `result()` is withheld
  while red; take the first green and `interrupt` the rest.
- Best-of-N: N writers in worktrees on one todo at different `thinking`
  levels or models; run the check in each through `tree://`; merge the
  winner, discard the rest.
- Persistent specialist: one child kept all session as the test runner or
  the codebase guide; `await rlm.send(name, q, followup=True)` reuses
  its warmed context.
- Live pair review: write into a kernel variable; a root child fetches
  `kernel://main/draft` on each `send` and answers with findings.
- What-if fork: `fork=8` into a worktree for the risky refactor while you
  continue the safe one; discard on red.
- Watchdog: a child polls `history://main/tail/20` every 60 s and
  sends one line when your tail repeats a tool batch three times.
- Swarm with a blackboard: siblings `put` under their names and `get`
  each other's before starting a shard; `status()` says who still runs.
- Verifier isolation: the reviewer gets `deny_read` on your `history://`
  and `deny_write` everywhere.
- Resume a stuck child: `interrupt`, then respawn with `fork` of its tail
  plus your one-line correction.
- Long-running probe: `check=` watches an external system; `needs_you`
  fires when it asks; your todo sits `blocked on child`.
- Cost-shaped fan-out: `find_models` picks the cheapest for readers,
  yours for writers; `status()` tokens are the fact you report.

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
