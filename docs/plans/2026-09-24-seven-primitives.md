# Yi: seven primitives for agent work at any scale

```
status:  PROPOSAL, 2026-09-24. Consolidates the 2026-09-23/24 design sessions: the hive
         revision, channels, the pressure test, and eight rounds of owner decisions.
         Extends docs/plans/2026-09-01-hive.md. Nothing lands without its D-rows (§15).
         The owner's words are quoted verbatim, never paraphrased: §6 says why. Method:
         the yi-ideate skill.
tree:    claude/yi-os-mailbox @ 38854e43 (0.294.0, last decision row D232), the top of
         the operating-system stack #427 → #467 → #483 → #494, not yet on main.
marks:   ✓ exists on that stack · ✚ new in this proposal
```

## 0. Summary

Every coding agent today, Claude Code included, treats a conversation as the unit of
work. State lives in a context window. Children return a paragraph and vanish.
Orchestration is either the model's judgment or a separate script, and nothing runs
unless someone is watching.

Yi becomes something else: **seven primitives, and everything else composed from them.**

| primitive | what it is | one line |
| --- | --- | --- |
| **Address** | a URL for anything | `scheme://owner[@node]/path`: memory, history, files, plans, channels, blobs, agents |
| **Todo** | the unit of work | state, intent, gate, lease, placement, result, for every piece of work from a typo up |
| **Agent** | a process that does todos | session + kernel + mailbox, with a lease and a wall, on a node |
| **Node** | where work can run | a card listing capacity, power, availability, isolations and price. One computer is a hive of one |
| **Channel** | how data moves | a durable ordered log with subscriptions and adapters |
| **Lease** | what work may spend | tokens, dollars and time, inherited downward, only ever shrinking |
| **Wall** | what work may touch | denied paths, URLs and channels, inherited downward, only ever narrowing |

One composite gets its own name: a **plan** is a graph of todos plus the program that
declared them plus its journal. It is used for non-trivial work only.

The same seven primitives run a one-line fix on a single laptop and an org-wide migration
across a thousand workers. Nothing switches modes between those two cases; only the
number of nodes and the size of the leases change.

## 1. The problem

### 1.1 Case study: Claude Code, observed in the session that produced this proposal

| limit | what happened | what removes it |
| --- | --- | --- |
| no persistent execution state | the shell's working directory was reset to the project root after every command that left it | the kernel: state lives across turns and is readable by address |
| children return a paragraph, then vanish | a research subagent read the 3,094-line OS plan and returned about 1,500 words; everything else it learned was unreachable | children stay addressable: `kernel://child/var`, `history://child/...` |
| context is one linear window | a 43 KB tool output spilled to a file that had to be re-read; earlier turns survive compaction only as a summary | history is addressable, paged and searchable; context is a list of addresses |
| runs only while watched | woken by a few harness events (task completion, PR CI, scheduled wake-ups), never by an arbitrary webhook, log or alarm | channels, adapters, suspend and activation |
| fixed placements | subagents run locally, in a git worktree, or in the vendor's cloud | nodes and isolation |
| orchestration is a second language | the Workflow tool is a JavaScript runtime separate from the work, with a guideline of under ten agents and resume only within one session | plans are Python in the same kernel, durable across restarts |
| the owner's words were overwritten | the owner named the primitive "channel"; the assistant renamed it "signal", then "topic". Told to "drop the biology", its next answer kept the biology nouns | the intent record and two-way traceability (§6) |
| failures found only when asked | asked "what else have you made mistakes on?", it found 13 in one pass: the same model, whose failures surfaced only once asked | boundary checks: the owner's question, asked for them (§6.4) |

### 1.2 Case study: Codex, OMP, Pi, OpenCode

Studied from their docs under `ref/agents/`, with cited file lines in the session record.

| | Codex | OMP | Pi | OpenCode |
| --- | --- | --- | --- | --- |
| subagents | `spawn_agent`; forked history; at most 6 at depth 1 | `task`; mailbox hub; at most 32 at depth 2 | none, by design | `task`; depth 1 |
| results | status JSON with the final text | text plus schema-validated output; `agent://id` | — | one text block |
| persistent interpreter | a PTY that persists by session id | eval kernels (Python, JS, Ruby, Julia) | none | not found |
| placement from the spawn tool | none; Codex Cloud is a separate surface with pick-by-hand best-of-N | `isolated` directories (copy-on-write where the filesystem allows); Docker only via robomp | none; container recipes in the docs | none |
| waking on external events | none inside the agent | robomp webhook queue; extension `triggerTurn` | extensions only | GitHub Actions, Slack, cron via CI |
| workflow engine | none | model-written `agent()`/`parallel()` code | none | none |

**What all four share:**
1. The model's own loop is the only orchestrator.
2. A child starts from text.
3. Results come back as text in the parent's context.
4. No spawn tool places a child in a container, VM or cloud.
5. Nothing inside the agent can wake it.
6. Everything runs in one process on one host.
7. The agent has no tool to search its past sessions.
8. Compaction loses detail that nothing restores.

OMP comes closest to Yi (it has `agent://` and `history://` addresses, eval kernels and a
mailbox hub), and Yi already borrowed from it. The gap left is durability, placement,
intent and verification.

### 1.3 The two failures underneath

**Intent loss.** The owner's working loop, in their words:

> 8) plan implementation is done. some important things were forgotten
> 9) i ask ai if the plan is done and everything was done correctly
> 10) immediately ai can recognize things were not done correctly. even such simple
> questions from myself can trigger this recognizal. this is my biggest frustration.

This loss has two causes:
- **The telephone effect.** Each hop restates the one before it. The plan paraphrases the
  brainstorm, the todos paraphrase the plan, a small child model gets a paraphrase of a
  paraphrase, and the judge gets a rubric, a fourth paraphrase.
- **Drowning.** The owner's short messages are a tiny share of a context full of system
  prompts, tools, rules and tool output.

**Verification that grades the restatement.** In the owner's words, judges are "so
fucking tunnel visioned on the 'rules' rather than the quality of the output". The
target they set is:

> is this the platonic ideal of the extrapolated intent from the user

## 2. Laws

1. **Scale-invariant.** "the primitives and foundation should be applicable to personal
   agent or scale to massive scale". There are no modes.
2. **Compose, don't add.** A feature that is not a composition of the seven primitives
   needs a D-row explaining why.
3. **The owner's words are the source of truth.** Intent travels by reference, never by
   restatement.
4. **One ledger.** "the jsonl ledger is the ledger. dont introduce multiple sources of
   truth". Every other store of session facts is a view or a query.
5. **Triggers and routing read data.** Models generate. Judges accept or reject, and may
   route on their verdict. That last clause is the one amendment to the standing rule,
   and it needs its own D-row.
6. **Recovery never runs saved source** (OS plan §8). A wait lives in the plan graph, not
   in a stack. A scheduled run of a saved definition is a new run, not a recovery.
7. **Permissive within the lease.** "start with a permissive model along the 'anything
   with a lease' and when we start running into permissions/authentications oversteps,
   then we add scopes". Every outward action is visible, and one command stops everything.
8. **Efficiency is the measure.** "efficiency. speed. performance. no wasted effort. no
   wasted time." It is counted as token yield on accepted work, but "dont over optimize
   for token yield that may result in half done, rushed work".
9. **Caps are fuses.** Eight workers per parent, sixteen members per family, depth three.
   Width comes from more roots, never from deeper or wider trees.

## 3. The seven primitives

### 3.1 Address

```
scheme://owner[@node]/path[?query]
```

| address | names |
| --- | --- |
| `kernel://reviewer@forge/findings` | a live Python object in another agent's memory, on another machine ✓ (D164 within a family; `@node` ✚) |
| `history://parent/since/412` | a slice of a transcript ✓ |
| `tree://sibling/src/parser.rs` | a file in a sibling's worktree ✓ |
| `plan://cve-1234/todos/repo-17` | a todo's state, contract and verdict ✓ |
| `family://known_breakages` | the family blackboard ✓ |
| `user://<session>/<n>` | the owner's message n, verbatim ✓ scheme, ✚ as intent |
| `channel://ci/apex?last=5` | the last five messages on a channel ✚ |
| `store://sha256:9f2c…` | any blob by content hash ✚ |
| `agent://reviewer@forge` | an agent: its card, state and mailbox ✓ scheme, ✚ `@node` |
| `node://forge` | a node's card ✚ |

Addresses already carry a durability class (`Durability::{Ephemeral, Durable}` in
`types/src/url.rs`), so a durable record cannot hold an address that dies with a
process.

Verbs:
- `fetch(url, offset, limit)` ✓: read anything, paged.
- `find(query, within=pattern)` ✚: search anything and get back **addresses**, so a
  search hit is already context.

### 3.2 Todo: the unit of work

"TODOs for everything. plans for non-trival only". Today there are two todo types. The
session list's `TodoItem` (`types/src/todo.rs:116`) has id, label, state, blocked-on,
note, evidence and children. The plan `Todo` (`types/src/plan/doc.rs:234`) has label,
edges, state, delegation, contract, attempts and retries. They merge ✚ into one:

```
Todo {
  id, label
  state:   Pending | Running { by, lease } | Blocked { on, note } | Done { output } | Failed { cause } | Cut
  on:      User { question, options? } | Child(AgentId) | Channel { address, filter }   ✚
           | Partition { node, since } ✚ | External { probe }
  after:   [TodoId]                  edges; empty in a flat list
  intent:  [Address]                 ✚ the intent record (§6.1)
  gate:    Contract + judge + class  ✓ contract, ✚ exam v2 and judge (§7)
  work:    run = code | delegate = Role { isolation, node?, model, wall, context }
  lease:   Lease                     ✓
  output:  Address                   results are addresses
  attempt, retries, refusals, contract_hash, children (grouping), evidence, extra
}
```

- A session's todo list is todos with no edges.
- A plan is todos with edges, a program and a journal.
- A child agent is a todo delegated to a new agent.
- A task-queue item is a ready todo.
- An approval, a preview, a durable wait and a timer are each a todo blocked on something.

The merge is a schema change to durable data, so it gets a version bump, an idempotent
migration and committed before/after fixtures (YI_DESIGN §19).

### 3.3 Agent

An agent is a process with an address:

- a **session**: the JSONL, which is the ledger ✓;
- a **kernel**: a persistent IPython memory whose variables are addressable ✓;
- a **mailbox**: a channel with one reader ✚ (today, D230's ordered queue ✓);
- a **lease** and a **wall** ✓;
- a **node** and an **isolation** ✚.

Services are agents with stable names and restart intensity (`rlm.service` ✓). Every
agent publishes a **card** (address, role, model, node, state, load) on the registry
channel ✚. With the owner's chosen default, any agent may read any other agent of the
same owner, and walls take that access away per child.

### 3.4 Node: where work can run

"not everyone has a home server. most users are 1 computer only. but this node approach
to resource management is a powerful architectural primitive".

```jsonc
{ "name": "laptop", "always_on": false, "power": "battery",
  "capacity": { "cpus": 10, "mem_gb": 32, "slots": 8 },
  "isolation": ["worktree", "container"], "price_per_hour": 0, "reach": "local" }

{ "name": "forge", "always_on": true, "power": "ac",
  "capacity": { "cpus": 16, "mem_gb": 64, "slots": 24 },
  "isolation": ["worktree", "container", "vm:proxmox/*"], "price_per_hour": 0, "reach": "direct" }

{ "name": "aws", "always_on": true, "elastic": true,
  "isolation": ["container:fargate/*", "vm:ec2-spot/*"], "price_per_hour": "per template" }
```

- **One computer is a hive of one.** Its card is computed locally from the machine. The OS
  plan's capacity counters (workers, worktrees, concurrent model requests, width
  `clamp(cores−1,1,8)`) become that node's capacity. Admission reads the card. Nothing
  about a single-computer install changes except that the counters now have a name.
- **A cloud account is an elastic node.** Its capacity grows through its driver, and it
  has a price.
- **Placement** is a choice of (node, isolation), made by a rule that reads cards and the
  todo's lease (§8.6).
- Cards live on `channel://nodes`, compacted so each node keeps only its latest card.

### 3.5 Channel: how data moves

A channel is a named, durable, append-only log. It has:
- **one home node**, which assigns offsets, so order is total within a channel and
  undefined across channels, and no consensus is needed;
- a **retention** policy, required;
- **key compaction**, optional;
- messages of at most 16 KiB, with anything larger passed by `store://` reference.

A **subscription** has a filter, batching (`batch {size, window}`), a `min_interval`, a
consumer `group`, and a durable offset. A match delivers to one of three targets:
- an **agent**: into its run queue (D230's `Queued { message, wakes, news }`), the only
  integration point with the run loop;
- a **todo**: it unblocks a todo blocked on that channel;
- a **definition**: it starts a run of a saved workflow.

Delivery is at-least-once, idempotent by message id, and presented once per subscription.
Overflow becomes an explicit gap message, never a silent drop.

**Adapters** are the only way in or out. Each is a supervised subprocess speaking JSON
lines on stdin/stdout, chosen by URI scheme from `~/.yi/adapters/<scheme>/`. A source is
acknowledged only after the home has written to disk. The only built-in is `clock`.

**Views versus logs.** A channel that mirrors session state is a projection of the JSONL,
never a copy: `session/<id>/events` and `log/outward` are examples. A channel carrying
outside data (CI events, alarms, mail) is its own log, because that data has no other
home. That is how channels keep law 4.

### 3.6 Lease

A lease holds tokens, a deadline ✓, and dollars ✚. It is drawn from the parent's lease at
spawn and is only ever smaller. Unspent budget returns at reap. `parent_close` (terminate
or request_cancel; abandon refused) ✓ matches Temporal's parent-close policy. Two things
are new:
- a **verification share** per intake class;
- **model quota** leased like any other resource, so parallel workers don't trip provider
  rate limits.

An exhausted lease suspends a session rather than killing it.

### 3.7 Wall

`deny_write`, `deny_read` and `deny_url` ✓, plus `deny_channel` ✚. A wall is inherited
downward and can only narrow. The exam's held-out probes (§7.1) sit behind the writer's
own `deny_read`, so no new mechanism is needed to hide them.

### 3.8 The composite: Plan

A plan is todos plus `after` edges, plus the recorded program that declared them, plus
its journal. It is used for non-trivial work only. Everything below exists:
- `Plan.create(goal, request_id=…)`: a re-run cell opens the same plan, which is
  Temporal's workflow ID;
- `todo(key, …)`: the same declaration returns the handle, and a changed one raises
  `SpecDrift`, which plays the role of Temporal's non-determinism error;
- `decompose`, `supersede`, `resume` (reattaches to state and never runs source);
- the shapes `fork_join` and `scatter`, and the recipe `review_pod`;
- the procedural graph `prompts/graph.json`.

## 4. Composition: everything else is built from the primitives

| capability | composed from | status |
| --- | --- | --- |
| mailbox | channel `agent://x/mail` with one reader | ✚ replaces the separate delivery path |
| cron / heartbeat | `clock` channel + subscription → wake an agent or start a definition | ✚ replaces the job store |
| recurring workflow | saved, content-hashed plan definition + clock subscription | ✚ |
| event-driven work | adapter channel + subscription → create a todo | ✚ |
| one-off task | a todo (trivial) or a plan (non-trivial), created by your prompt | ✓ |
| durable wait / signal | todo `Blocked { on: Channel { filter } }` | ✚ generalizes `External { probe }` |
| durable timer | todo blocked on the `clock` channel | ✚ |
| approval | todo `Blocked { on: User }` | ✓ |
| preview | todo `Blocked { on: User { options: 3–5 with previews } }`, via `ask_user` | ✚ |
| child agent | todo delegated to a new agent on a node | ✓ worktree, ✚ other isolations |
| task queue | ready todos + a consumer group of agents; a claim is a start with a lease | ✚ |
| re-dispatch | lease expiry → `Pending`; a late result is refused by its attempt id | ✓ `AttemptId` |
| partition | todo `Blocked { on: Partition { node } }` for the rest of its lease | ✚ |
| best-of-N | N todos with one intent record, compared by the intent judge | ✚ |
| intent record | the owner's words + exemplars, as addresses on a todo | ✚ |
| standing preference | the address of the owner's verbatim words | ✚ replaces model-written memory notes |
| gate | contract + intent judge on a todo | ✓ contract, ✚ judge |
| discovery | registry channel (compacted) of agent and node cards | ✚ |
| search | `find` over addresses | ✚ generalizes `history.grep` ✓ |
| remote attach | subscription to `session/<id>/events` from an offset | ✚ |
| kill switch | message on the control channel; the host obeys without asking a model | ✚ |
| spend alert | subscription on lease events | ✚ |
| outward-action log | projection of the JSONL: every tool call with an outward effect | ✚ view, not a store |
| merge queue | channel + consumer group of one + a check | ✚ |
| review pod, scatter, fork/join | plan shapes | ✓ |
| resource management | node cards + lease admission | ✚ names what exists |

## 5. Work: one shape for every kind

### 5.1 Three independent axes

Every piece of work is a todo or a plan. What distinguishes one kind of work from another
is three independent choices, not three systems:

| | node bodies are code | node bodies are models |
| --- | --- | --- |
| **static graph** (declared before running) | nightly backup and checksum: `run=` todos only, zero tokens | the 9 a.m. triage: fixed steps, model bodies |
| **dynamic graph** (grows while running) | one todo per failing test, computed by code from data | "fix this bug": the model decomposes as it learns |

The third axis is the **trigger**, and any cell above can have any trigger:
- a **prompt** makes it one-off;
- the **clock** makes it recurring;
- a **channel** makes it event-driven.

Four things hold in every cell: routing and triggering read data, every todo passes its
gate, the journal records everything, and recovery reads state.

### 5.2 Temporal, mapped

| Temporal | Yi | |
| --- | --- | --- |
| Workflow ID | `Plan.create(request_id=)` | ✓ |
| Workflow definition | plan program (Python) | ✓ |
| Activity | todo: `run=` code or `delegate=` agent | ✓ |
| Event history | plan journal + session JSONL | ✓ |
| Non-determinism error | `SpecDrift` on a changed declaration | ✓ |
| Retry policy, timeouts, heartbeat | `retry` with a cap, lease deadline, progress mail | ✓ |
| Child workflow + ParentClosePolicy | delegated todos + `parent_close` | ✓ |
| Query / Update | `fetch("plan://…")` / `rlm.request` | ✓ |
| Continue-as-new | `Plan.supersede`, compaction | ✓ |
| Signal | a message that unblocks a todo | ✚ |
| Durable timer | todo blocked on `clock` | ✚ |
| Task queue + workers | ready todos + consumer group | ✚ |
| Schedules (+ overlap policy) | clock subscription; overlap policy from today's `should_defer` | ✚ |
| Visibility / search attributes | registry + `find` | ✚ |
| Saga | a failed todo triggers compensating todos | recipe |

**The one deliberate difference: where a wait lives.** Temporal keeps a waiting workflow's
position in its code, and after a crash it rebuilds that position by replaying the code
against the history. That requires deterministic workflow code. Yi's plan programs are
model-written Python with imports, files and randomness, which is why the OS plan made
"recovery never runs saved source" a law. So in Yi a wait is a **todo blocked on a
channel**: the daemon holds the subscription, and a match unblocks the todo. Recovery
reattaches to state, and there is no stack to rebuild.

What that buys:
- A plan can wait for days at $0.
- A crash at hour 9 of 12 resumes with every accepted result intact, and repeats no model
  call.
- An activity whose outcome is unknown is surfaced for a decision, never retried blindly.
  An agent editing a repo is rarely idempotent, so Temporal's retry default doesn't fit.
- There is no server cluster to run.

### 5.3 Example: every weekday at 9 a.m.

```python
# ~/.yi/workflows/morning.py: a saved definition; every run records its content hash
from yi.plan import Plan
from yi.roles import Reader, Writer

async def morning(fired_at: str) -> None:
    plan = await Plan.create("morning triage", request_id=f"morning-{fired_at}")  # one run per firing
    await plan.todo("inbox", run=pull_inbox)                         # code: mail since the last run
    await plan.todo("triage", after=["inbox"], delegate=Reader())    # model: what needs me, and why
    await plan.todo("replies", after=["triage"], delegate=Writer(accept=REPLY_EXAM))   # ✚ exam (§7)
    await plan.todo("brief", after=["triage", "replies"], delegate=Writer(accept=BRIEF_EXAM))
    await plan.run(budget="20m")

await rlm.subscribe("clock://0 9 * * 1-5", start="workflow://morning")   # ✚ the trigger
```

- The definition's intent record (your words from when you set it up, and the chosen
  preview) travels with it, so every run is judged against what you asked for, not
  against a restatement.
- Editing the file makes a new version. The next run uses it, and each run records the
  hash it ran.
- Sending replies is allowed within the lease and shows up in the outward-action log.

### 5.4 Example: an event creates a todo

```python
await rlm.subscribe("channel://ci/apex?conclusion=failure", create={        # ✚
    "label": "make main green at {sha}",
    "intent": [LINTER_PREF],                   # the address of your own words about linters
    "delegate": {"role": "writer", "isolation": "container:rust:1.91", "accept": "just check"},
    "lease": {"tokens": 400_000, "deadline_s": 3600},
})
```

## 6. Intent and taste

### 6.1 The intent record

The intent record is a list of addresses on a todo. It is not a new store. It contains:

- **your words in the task**, verbatim: every message from the task's start, your answers
  and your picks;
- **your corrections and rejections**. These matter most, because "don't overengineer",
  "use my name" and "drop the biology" mark exactly where models drift back;
- **exemplars**: the previews you chose (§6.3), by `store://` hash;
- **standing preferences**: your exact words plus their ledger address. They replace
  model-written memory notes, which are themselves restatements.

Size is bounded without restating you:
- **Your own writing** stays verbatim up to a per-message limit. Past it, the start and
  end stay verbatim and the full text is one fetch away by address.
- **A paste** is material, not intent, and terminals report pastes separately (bracketed
  paste mode). It is stored by hash, carries a short summary in the record, and its full
  text is one fetch away.

**Every child and every judge receives the intent record verbatim.** Compaction may
summarize tool output and model reasoning; it never paraphrases you ✚. That one rule
removes the telephone effect at its source.

### 6.2 Two-way traceability

Every plan item cites the words in the intent record it serves. Every sentence of the
intent record is covered by an item, or waived with a reason. Both directions are
mechanical checks, run when the plan is drafted and after every revision.

- **A sentence with no item** is something about to be forgotten (step 8). It is caught
  before the work starts.
- **An item that cites nothing** is something nobody asked for, which is overengineering
  (step 3). It is cut before the work starts.

### 6.3 Previews: always, and light

"i think always previews, but not heavy previews. previews can be architecture diagrams,
mockups, etc. a pattern i have been recently using to great success is a multiple choice
option for 3 - 5 different styles".

A preview is `ask_user` with 3–5 options, each carrying a light preview: an architecture
diagram, a mockup, a sample paragraph, an API sketch, or one finished slide. That makes
it a todo blocked on the user. Who picks depends on who's there:

- **you**, when you're present;
- **the parent**, inside a child, because M2 routes `ask_user` to the parent;
- **the calibrated judge** (§6.5), when the run is unattended. The losing options are
  kept, so you can overrule it later.

The pick and every rejected option join the intent record. The pick becomes the
**exemplar** that every later stage is compared against. "sometimes i dont even know what
i want until i see it", so the preview shows it before anything commits to a direction.

### 6.4 Boundary checks: your question, asked for you

At every boundary, the host asks the intent judge your question: "Is this what they
meant? What would they object to first?" The boundaries are:
- the plan is drafted;
- the plan is revised after a discovery (step 7);
- a child finishes;
- done is claimed (step 9, automated).

The trigger is the boundary, which is an event, not a model's choice. The verdict routes
the work back with the specific mismatch.

### 6.5 The intent judge

- **It reads** the intent record verbatim and the exemplars.
- **It sees what you'd see**: the rendered page rather than the code, the slides rather
  than the XML.
- **It compares instead of scoring**: against the exemplar, the previous version or the
  alternatives. "Which is closer to what they meant?" is more reliable than a number, for
  models as for people.
- **It predicts your reaction**, meaning your first objections, instead of grading a
  checklist.
- **It is calibrated on you.** Past boundaries where you reacted are already in the JSONL
  as approvals, rejections, corrections and "you're framing this wrong". Candidate judges
  are replayed on those moments. The owner's choice is to use the **cheapest judge that
  predicts your actual reactions well enough for the task class**. The calibration is a
  query, not a store.

### 6.6 Taste stays close to your words

Decisions that carry taste (direction, tone, structure, scope) are made by an agent that
reads the intent record directly, on a strong model. Children get bounded mechanical work,
with the intent record and the exemplar passed by reference. A small model never rebuilds
your taste from a paraphrase.

### 6.7 Walkthrough: a landing page

1. **Intent record:** your six brainstorm messages: "calm, like Linear but warmer", "no
   gradients", "less copy".
2. **Preview:** a multiple-choice question with five hero mockups. You pick B: "B, but less
   copy." B becomes the exemplar.
3. **Plan:** every item cites your words, and "warmer" and "less copy" are both covered. An
   animated testimonial carousel cites nothing, so it's cut.
4. **Build:** section children get the intent record and exemplar B by reference.
5. **Done claimed:** the judge compares the rendered page with B and your words: "They said
   less copy; the features section has 140 words, B had 40." The fix is sent back before
   you look.
6. **Your review:** it looks like B.

## 7. Gates

Models are good at generating candidates and bad at judging. Mechanical checks are good at
judging and can't generate. So a gate makes the model search and lets the host decide.

### 7.1 The exam

At intake, the task becomes probes. Each probe has:
- a question or check;
- the expected answer;
- a decider: command ✓, schema ✓, example ✓, blind reader ✚ or refuter ✚;
- a weight and a critical flag.

The exam is frozen by content hash (`freeze` ✓). The writer sees the requirements and most
of the probes. **A held-out share stays behind its wall**, the way Terminal-Bench keeps its
verifier in a separate container. Every probe cites the intent sentence it tests (§6.2).

### 7.2 Blind consumer tests

An artifact passes if someone can use it. A fresh agent, with no access to the writer's
context and only the rendered artifact in front of it, does the reader's job:
- a deck: answer the probes from the slide images;
- an email: extract the ask and the deadline;
- a README: follow it in a clean container until the command works.

Answers are compared with the frozen expectations: exact, numeric or set comparison where
possible, and a single-probe judge otherwise. A failure names the probe, so the fix is
targeted. This is QA-based evaluation (as in QAGS and QuestEval), and it is what tests
already do for code.

### 7.3 Refuters search; the host verifies

Refuters hunt for counterexamples to each requirement. A finding counts only with evidence
the host can check mechanically:
- a quote at a location (`verify_quotes` ✓);
- a command that reproduces;
- a number recomputed from its cited source.

Unverified findings are dropped.

### 7.4 Claims carry addresses

Every number, fact and "tests pass" in an output cites an address: a ledger row, a kernel
variable, a `file:line`, or a command. The host fetches and recomputes. In the session
behind this proposal, a wrong row count and an inflated trial-hours estimate would both
have failed here.

### 7.5 Coverage through decomposition

When a todo splits, every parent probe must be owned by a child or kept by the parent
(`covers` ✓, D228). Children can't each finish their part while the whole misses
something.

### 7.6 Acceptance

**Gates and judge must both pass.** Probes guard correctness, and the judge guards intent.
Either failing sends the work back with its reason. After the retry cap, the work goes to
you, or waits in a queue when nobody's there.

The verification budget is set per intake class:

| class | gate | judge | example share of the lease |
| --- | --- | --- | --- |
| trivial | one command | sampled, 1 in N | 5% |
| standard | probes + one blind read | at done | 15% |
| large | + refuters | at every boundary | 25% |
| open-ended | + previews, you confirm when present | at every boundary | 35% |

The model sizes the class, and the rules bound it. The shares are data the host enforces,
and the ledger shows whether they were too much or too little.

## 8. Nodes and placement

### 8.1 Isolation on a node

The existing `isolation` keyword grows from `"worktree"` into a placement:

```python
isolation="worktree"                             # ✓ today
isolation="container:rust:1.91"                  # ✚ first: Docker or Podman on this node
isolation="vm:proxmox/rust-residence@forge"      # ✚ a VM on another node
isolation="container:fargate/rust@aws"           # ✚ an elastic node
```

### 8.2 Placement drivers

A placement driver is an executable with two verbs, `up` and `down`, one per isolation
kind. It starts `yi serve` inside the environment, which reaches home over the `Wire`
transport. On the same node, that transport is a Unix socket; across nodes it is tailcat
first.
- **Workspace in:** a git ref plus content-addressed blobs.
- **Result out:** a diff with its gate verdict. `merge_worktree` ✓ becomes `merge(child)`
  for every placement.
- **Transparency:** the child can't tell where it runs. It has the same tools, the same
  `rlm`, the same lease and wall, and the same address.

### 8.3 Partition

When a node becomes unreachable, its todos become `Blocked { on: Partition { node, since } }`
for the rest of their lease. A child that returns in time continues. When the lease
expires, the todo goes back to `Pending` and is re-dispatched, and a late result is refused
by its attempt id.

### 8.4 Reads across nodes

Reads across nodes are open within one owner by default, and walls narrow them per child.
Every cross-node read appears in the outward-action view.

### 8.5 The default-placement question (open)

This question only exists once there is a second node. On one computer, everything is
local and the node card only governs admission.

| default | for | against |
| --- | --- | --- |
| **Local unless told** | predictable; no workspace shipping; works offline; code stays on the machine; simple to reason about | closing the lid pauses the work; battery and thermals; a laptop can't hold wide fan-out |
| **Nearest strong node** | survives sleep; faster machines; scales | remote without being asked for; uncommitted local edits must ship; work diverges if you keep editing locally; depends on the network; a paid node would mean surprise cost |

A third option for the owner to weigh: **placement follows lifetime and capacity.**
- **Attended work** (a live client, a short lease) runs on the node you're at.
- **Unattended work** (scheduled runs, detached children, leases longer than the session)
  goes to an always-on, free node with the right isolation, if one exists.
- **Work that exceeds the current node's card** goes to a node that can hold it.
- **A priced node** is used only when a todo or workflow names it.

On one computer, all three defaults reduce to "local". The owner hasn't decided (§15).

## 9. Scale

- **Width comes from roots.** The caps stay as fuses. A thousand workers are a thousand
  ordinary roots, each within its own caps, coordinated through channels.
- **A task queue is ready todos plus a consumer group.** A worker's claim is a `start`
  with a lease; its submission is the gate; an expired lease returns the todo to
  `Pending`. The queue needs no job type of its own, because its items are todos.
- **Workers are placed by driver on nodes chosen by card.** Examples: 8 containers on a
  laptop, 24 on the forge, 200 on an elastic node, each under a lease drawn from the
  effort's lease.
- **Model quota is leased**, so parallel workers share provider limits instead of all
  hitting rate limits at once.
- **Results come back as addresses**; diffs go through a merge queue; the control channel
  pauses, drains or cancels everything.

```python
plan = await Plan.create("patch CVE-2026-1234 org-wide", request_id="cve-2026-1234")
for hit in await rlm.find("openssl-sys < 0.10.70", within="mcp://github/apex"):     # ✚
    await plan.todo(hit.repo, intent=INTENT,                                          # ✚
        delegate=Writer(accept="cargo test && cargo deny check advisories",
                        isolation="container:rust:1.91"))
await plan.run(shape=queue(group="patchers"), budget="12h")                          # ✚
```

What limits scale: money, provider rate limits, verification throughput, merge contention
and human attention. The design makes each one visible, and nothing else should be in the
way.

## 10. Safety on day one

The owner chose three day-one protections, and each is a composition:
- **Outward-action log**: a projection of the JSONL, covering every send, push, PR, cloud
  call, cross-node read and spend, with a live tail.
- **Kill switch**: one command, or a message on the control channel, pauses or cancels
  every run on every node. The host obeys it without asking a model.
- **Spend alerts**: a subscription on lease events with thresholds.

Scopes come later, when the log shows the first overstep.

Channel data is the new way untrusted input reaches models, and it gets four defenses:
- filters run before any model sees a message;
- external sources default to batched delivery;
- every wake costs lease;
- every message is rendered with its source, as data and never as instructions.

## 11. Measuring efficiency

- **Token yield:** tokens and dollars of accepted work, divided by the total.
- **Accepted:** the gates and the judge passed, and the JSONL shows no later revert or
  rewrite. Rushed work that gets undone becomes waste after the fact.
- **Every todo measures itself** from its own record: the lease gives cost, the journal
  gives time, and the gate and judge verdicts plus your later actions give acceptance.
- **The baseline is the existing session corpus.** No special baseline workflow is needed.
- **Waste is itemized:** failed attempts, empty wakes, loops, duplicated discovery, and
  verification spent beyond its class share.

## 12. What exists and what's new

| exists on the branch | where |
| --- | --- |
| `rlm.run/service/send/request/followup/wait/result/status/fetch/put/get/ls/revoke/merge_worktree` | `python/yi_runtime/src/rlm/__init__.py` |
| URL schemes and durability classes | `crates/types/src/url.rs` |
| `Plan`, `todo`, `decompose`, `resume`, contracts, `Writer`/`Reader`, shapes, `review_pod` | `python/yi_runtime/src/yi/` |
| plan `Todo` with `Delegation.context: Vec<Url>` | `crates/types/src/plan/doc.rs:217-262` |
| session `TodoItem` | `crates/types/src/todo.rs:116` |
| ordered per-session queue (D230) | `crates/runtime/src/session/run.rs` |
| leases, `ParentClose` | `crates/runtime/src/lease.rs`, `crates/types/src/lease.rs:80` |
| walls | `crates/runtime/src/wall.rs` |
| heartbeat scheduler | `crates/runtime/src/schedule/mod.rs` |
| probe ladder | `crates/runtime/src/plan/probe.rs` |
| `history.grep` | `crates/runtime/src/wiring.rs:266` |
| procedural graph | `crates/runtime/src/prompts/graph.json` |

New in this proposal:
- **primitives:** the merged todo, channels with adapters and `clock`, nodes and their
  cards, placement drivers and isolation URLs;
- **addressing:** `@node` and `find`;
- **intent:** the intent record, the compaction rule, standing preferences as quotes,
  previews through `ask_user`;
- **gates:** exam v2 (held-out probes, blind readers, verified refuters, cited claims),
  the intent judge and its calibration query;
- **network and safety:** the `Wire` over tailcat, the kill switch, spend alerts, and the
  outward-action view.

## 13. What this deletes (one in, one out)

- the heartbeat job store and service; the cron parsing moves into the `clock` adapter;
- the probe ladder's polling loop, which becomes an `exec` adapter plus a todo blocked on
  a channel;
- the mailbox's separate delivery path, since mail becomes a channel;
- the session `TodoItem` type, merged into the one todo;
- model-written memory notes, replaced by verbatim quotes with addresses;
- from the hive plan: the custom cursor protocol, the idempotency frame field, the
  drop-box ingest, the `WakeCapsule` job and the anchor wake queue.

These deletions are proven by a falling `src/` count on the size ratchet.

## 14. Build order

Each stage is demoable and has its own D-row and issue. The first two need no new
infrastructure and go after the owner's biggest frustration first.

| stage | scope | demo | gate |
| --- | --- | --- | --- |
| **1. One todo, your words kept** | the todo merge (migration + fixtures); the intent record; compaction never paraphrases you; standing preferences as quotes; two-way traceability | a long session whose early messages survive compaction verbatim; a plan that flags a forgotten sentence and cuts an item nobody asked for | the schema fixtures before and after; the traceability check red on a planted omission |
| **2. Previews and judges** | `ask_user` with 3–5 previews; the intent judge at boundaries; exam v2; both must pass; class budgets; the calibration query | the landing page of §6.7 | the judge catches a planted drift; the calibration query ranks judges on the existing corpus |
| **3. Channels** | mailbox and clock on channels; todos blocked on a channel; kill switch; spend alerts; outward-action view | the 9 a.m. workflow; a plan that waits two days for approval at $0 | the eight `mbx-*` trials stay green; the line count falls |
| **4. Nodes and containers** | node card for one computer; `isolation="container:…"`; admission by card | eight container children on one laptop | the result and merge path are identical to worktrees |
| **5. Adapters** | `exec`, `file`, `github`, `aws+sqs` | CI red → todo → verified fix | the probe ladder retires |
| **6. Many nodes** | `@node` addresses, tailcat `Wire`, registry, `find`, partition handling | `fetch("kernel://reviewer@forge/findings")` from a laptop; close the lid and work continues | the hive plan's O1 drive script |
| **7. Elastic nodes and queues** | cloud drivers, consumer groups, model quota | a 1,000-todo effort, with its ledger | cost per accepted todo reported by query |

## 15. Decisions and open questions

**Owner decisions, 2026-09-24:**

| topic | decision |
| --- | --- |
| audience | "the primitives and foundation should be applicable to personal agent or scale to massive scale" |
| recovery | keep the law; waits live in the graph |
| autonomy | permissive within the lease; scopes after the first overstep |
| day one | outward-action log, kill switch, spend alerts |
| efficiency | token yield, not at the cost of "half done, rushed work" |
| ledger | "the jsonl ledger is the ledger" |
| daemon | always on |
| channels reversal | staged, not rejected |
| paying for features | the absorbed machinery |
| first placement | local containers |
| acceptance | "contract passed is bare minimum"; the judge is framed on "the platonic ideal of the extrapolated intent" |
| judges | may route |
| intake depth | the model sizes it, rules bound it |
| verification budget | per intake class |
| hidden probes | a held-out share |
| judge model | the cheapest that passes calibration |
| intent record | all of your words, bounded; pastes stored as material |
| standing preferences | quote plus address |
| previews | always, light, 3–5 styles as multiple choice |
| unattended picks | the calibrated judge |
| gates vs judge | both must pass |
| two people | the task's owner wins |
| unit of work | "TODOs for everything. plans for non-trival only"; the todo types merge |
| trivial todos | judge sampled |
| cross-agent reads | open within one owner, walls narrow |
| workflow versions | new version, recorded per run |
| partition | wait on the lease, then re-dispatch |
| promotion to workflows | "overengineering for now" |
| nodes | "most users are 1 computer only. but this node approach to resource management is a powerful architectural primitive" |

**Open:**
1. The default placement once there is a second node (§8.5).
2. Power policy on battery: should background todos wait for AC power?

**D-rows needed before code:**
- channels: reverses OS plan §3.4's cuts of pub/sub topics and cross-root messaging,
  "staged, not rejected";
- the todo merge: a schema version bump, a migration, and fixtures;
- judges may route: amends the deterministic-trigger rule;
- the intent record and the compaction rule;
- the always-on daemon;
- nodes and placement URLs;
- previews through `ask_user` options;
- the YI_DESIGN §1.1 edit for one-in-one-out.

## 16. Not building

- **promotion of repeated work into workflows**: the owner's call, "overengineering for now";
- **Temporal-style replay of saved source**;
- **a workflow engine, flow language or cluster scheduler**: plans stay Python, and
  placement stays a driver;
- **CloudEvents as the internal format**: conversion happens at the adapter boundary;
- **a second store for signals, feedback or memory**: the JSONL is the ledger;
- **permission scopes**: until the log shows the first overstep;
- **new model tools**: everything lives on `rlm` and `yi`, and the fixed prompt prefix
  stays unchanged.
