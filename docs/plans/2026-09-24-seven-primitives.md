# Yi: seven primitives for agent work at any scale

```
status:  PROPOSAL, revision 2 (2026-09-26). Revision 1 (2026-09-24, #504) consolidated
         the 2026-09-23/24 design sessions: the hive revision, channels, the pressure
         test, and eight rounds of owner decisions. Revision 2 answers an adversarial
         review (§17) and the owner's four decisions on it. Extends
         docs/archive/plans/2026-09-01-hive.md. Nothing lands without its D-rows (§15). The
         owner's words are quoted verbatim, never paraphrased: §6 says why. Method:
         the yi-ideate skill.
tree:    main @ 36cd8eaa (0.319.0, last decision row D244). Every ✓ was re-read there.
marks:   ✓ exists on main · ✚ new in this proposal · ⏸ deferred
```

## 0. Summary

Every coding agent today, Claude Code included, treats a conversation as the unit of
work:
- state lives in a context window;
- children return a paragraph and vanish;
- orchestration is either the model's judgment or a separate script;
- nothing runs unless someone is watching.

Yi becomes something else: **seven primitives, and everything else composed from them.**

| primitive | what it is | one line |
| --- | --- | --- |
| **Address** | a URL for anything | memory, history, files, plans, channels, blobs, agents |
| **Todo** | the unit of work | state, intent, gate, lease and result, for every piece of work from a typo up |
| **Agent** | a process that does todos | a session, a kernel and a mailbox, with a lease and a wall, on a node |
| **Node** | a machine and what it admits | a card whose slots bound the live kernels and roots on that machine. One computer is a hive of one |
| **Channel** | how outside data moves in | a durable ordered buffer with subscriptions and adapters; it stops being authority once a message is delivered |
| **Lease** | what work may spend | tokens and time today, dollars later; inherited downward, only ever shrinking |
| **Wall** | what work may touch | denied paths and URL prefixes, inherited downward, only ever narrowing |

One composite gets its own name: a **plan** is a graph of todos, plus the program that
declared them, plus its journal. It is used for non-trivial work only.

The same seven primitives run a one-line fix on a laptop and an org-wide migration.
Nothing switches modes between those two cases; the number of nodes and the size of the
leases change.

Revision 2 is smaller than revision 1:
- Several ✓ marks were wrong and are now ✚.
- Two mechanisms revision 1 called compositions are named as new: a claim protocol and an
  overlap policy.
- The clock may no longer start saved code.
- The build order begins with a read-only experiment that can falsify the core bet
  before any schema change.

## 1. The problem

### 1.1 Case study: Claude Code, observed in the session that produced this proposal

| limit | what happened | what removes it |
| --- | --- | --- |
| no persistent execution state | the shell's working directory was reset to the project root after every command that left it | the kernel: state lives across turns and is readable by address |
| children return a paragraph, then vanish | a research subagent read the 3,094-line OS plan and returned about 1,500 words; everything else it learned was unreachable | children stay addressable: `kernel://child/var`, `history://child/...` |
| context is one linear window | a 43 KB tool output spilled to a file that had to be re-read; earlier turns survive compaction only as a summary | history is addressable, paged and searchable; context is a list of addresses |
| runs only while watched | woken by a few harness events, never by an arbitrary webhook, log or alarm | channels, adapters, suspend and activation |
| fixed placements | subagents run locally, in a git worktree, or in the vendor's cloud | nodes and isolation |
| orchestration is a second language | a JavaScript workflow runtime separate from the work, with a guideline of under ten agents and resume only within one session | plans are Python in the same kernel, durable across restarts |
| the owner's words were overwritten | the owner named the primitive "channel"; the assistant renamed it "signal", then "topic". Told to "drop the biology", its next answer kept the biology nouns | the intent record and two-way traceability (§6) |
| failures found only when asked | asked "what else have you made mistakes on?", it found 13 in one pass: the same model, which had not looked until asked | boundary checks: the owner's question, asked for them (§6.4) |

### 1.2 Case study: Codex, OMP, Pi, OpenCode

Studied from their docs under `ref/agents/`.

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

### 1.3 The two failures underneath

**Intent loss.** The owner's working loop, in their words:

> 8) plan implementation is done. some important things were forgotten
> 9) i ask ai if the plan is done and everything was done correctly
> 10) immediately ai can recognize things were not done correctly. even such simple
> questions from myself can trigger this recognizal. this is my biggest frustration.

Yi already keeps every real user message verbatim through compaction, within a 64,000-token
floor (`crates/context/src/floor.rs:5,41`). The loss happens elsewhere:
- **Restatement.** The summarizer writes the "Goal" and "Constraints & Preferences" sections
  in its own words (`crates/context/src/prompts.rs:9-12`). Plans paraphrase the brainstorm,
  todos paraphrase the plan, and a child gets a paraphrase of a paraphrase.
- **Drowning.** The owner's short messages are a small share of a context full of system
  prompts, tools, rules and tool output.

**Verification that grades the restatement.** In the owner's words, judges are "so
fucking tunnel visioned on the 'rules' rather than the quality of the output". The
target they set:

> is this the platonic ideal of the extrapolated intent from the user

## 2. Laws

1. **Scale-invariant.** "the primitives and foundation should be applicable to personal
   agent or scale to massive scale". There are no modes.
2. **Compose, don't add.** A feature that is not a composition of the seven primitives is
   named as a new mechanism, with its own D-row. It is never called a composition.
3. **The owner's words are the source of truth.** Intent travels by address, never by
   restatement.
4. **One authority per fact.** The owner said "the jsonl ledger is the ledger. dont
   introduce multiple sources of truth", and on 2026-09-26 chose "One authority per fact".
   - A session fact's authority is the session JSONL.
   - A plan fact's authority is the plan journal, `.yi/plans/<root>/ops.jsonl`
     (YI_DESIGN.md:532), which is already a second JSONL log.
   - A channel is an adapter buffer. It is not authority for anything once its message
     is delivered into one of those logs.
   - Every other store is a view.
5. **Triggers and routing read data.** Models generate. Judges accept or reject, and may
   route the work, but only through a closed verdict enum (§6.5). That is the one
   amendment to the standing rule.
6. **Neither recovery nor the clock runs saved source.** Recovery reattaches to state
   (OS plan §8). On 2026-09-26 the owner chose "Clock only creates a todo": a tick appends
   a todo or a wake, and a live agent decides what code to run.
7. **Permissive within the lease.** "start with a permissive model along the 'anything
   with a lease' and when we start running into permissions/authentications oversteps,
   then we add scopes".
   - On day one, any family member may read another member's kernel (D164). The log that
     would show an overstep is ✚, and a log is not a gate.
   - One command stops everything.
8. **Efficiency is the measure.** "efficiency. speed. performance. no wasted effort. no
   wasted time." It is counted as token yield on accepted work, but "dont over optimize
   for token yield that may result in half done, rushed work".
9. **Caps are fuses.** The existing fuses are 8 children per parent, a family cap of 16,
   and a depth that defaults to 1 with a lever ceiling of 3 (`crates/runtime/src/subagent.rs:25-30`,
   `crates/runtime/src/levers.rs:90`). They bound a family, not a machine. Roots are bounded
   by the node card's slots (§3.4).

## 3. The seven primitives

### 3.1 Address

✓ Today a `Url` is a scheme, a path and an optional hashline fragment
(`crates/types/src/url.rs:160-164`), with a durability class (`Durability`, `:22`).

✓ Schemes resolve today:
- `local://`, `kernel://`, `plan://`, `agent://`, `history://`, `checkpoint://`, `mcp://`
  and `user://`;
- `family://` and `tree://`, which parse as external schemes (`crates/runtime/src/fetch/schemes.rs:324,356`).

✚ New:
- an `owner@node` authority segment (⏸ deferred);
- `channel://` and `store://`.

| address | names | |
| --- | --- | --- |
| `kernel://reviewer/findings` | a live Python object in another agent's kernel (D164) | ✓ |
| `history://parent/since/412` | a slice of a transcript | ✓ |
| `tree://sibling/src/parser.rs` | a file in a sibling's checkout, walled by `deny_read` | ✓ |
| `plan://cve-1234/todos/repo-17` | a todo's state, contract and verdict | ✓ |
| `family://known_breakages` | the family blackboard | ✓ |
| `kernel://reviewer@forge/findings` | the same, on another machine | ⏸ |
| `channel://ci/apex?last=5` | the last five messages on a channel | ✚ |
| `store://sha256:9f2c…` | a blob by content hash | ✚ |

Verbs:
- `fetch(url, offset, limit)` ✓ reads anything, paged.
- `find(query, within=pattern)` ✚ searches and returns **addresses**, so a hit is already
  context. `history.grep` ✓ (`crates/runtime/src/wiring.rs:316`) is its first case.

### 3.2 Todo: the unit of work

"TODOs for everything. plans for non-trival only". Yi has two todo types today, with
different serializers and stores:

| | session todo | plan todo |
| --- | --- | --- |
| type | `TodoItem` (`crates/types/src/todo.rs:116`) | `Todo` (`crates/types/src/plan/doc.rs:234`) |
| stored in | `custom` entries in the session JSONL | the plan journal, `ops.jsonl` |
| ids | `t<n>` | labels |
| blocked on | a flat `BlockedOn` | `BlockedOn::{Child, User, External { probe }, Other}` (`doc.rs:74`) |
| other state | — | `after` edges, a delegation, a contract, `Running { by }` (`doc.rs:90`), `Abandoned` |

**Revision 2 does not merge them first.** The owner chose "Judge replay first", so stage
0 (§14) must hold before any schema change. Once it holds, the merge is a schema change
to durable data: a version bump, an idempotent migration, and committed before/after
fixtures (YI_DESIGN §19). Old rows keep their shapes: `Running { by }` gains an optional
epoch, and `Abandoned` stays `Abandoned`.

The target shape, one type used everywhere:

```
Todo {
  id, label
  state:   Pending | Running { by, epoch? ✚ } | Blocked { on, note } | Done { output, resolution }
           | Failed { cause, last } | Abandoned
  on:      User { question, options? ✚ } | Child(AgentId) | External { probe }
           | Channel { address, filter } ✚
  after:   [TodoId]                   edges; empty in a flat list
  intent:  [Address] ✚                the intent record (§6.1): addresses only
  gate:    Contract ✓ + intent judge ✚ + class ✚
  work:    run = code | delegate = Role { isolation, model, wall, context }
  output:  Address                    results are addresses
  attempt, retries, refusals, contract_hash, children, note, extra
}
```

- A session's list is todos with no edges.
- A plan is todos with edges, a program and a journal.
- A child agent is a delegated todo.
- An approval, a preview, a durable wait and a timer are each a todo blocked on
  something.

### 3.3 Agent

An agent is a process with an address:
- a **session**: the JSONL, the authority for its facts ✓;
- a **kernel**: persistent IPython whose variables are readable by address ✓ (D164,
  `python/yi_runtime/src/rlm/__init__.py:950`);
- a **mailbox**: D230's ordered queue ✓, read with `rlm.receive` ✓ (`rlm/__init__.py:677`).
  It stays an in-process queue plus the session JSONL, and it is not a durable channel;
- a **lease** and a **wall** ✓;
- a **node** ✚.

Services are agents with stable names and restart intensity (`rlm.service` ✓). A child
whose todo is blocked on the user already shows as `needs_you` to its family ✓
(`crates/runtime/src/family.rs:188`).

### 3.4 Node: a machine and what it admits

"not everyone has a home server. most users are 1 computer only. but this node approach
to resource management is a powerful architectural primitive". On 2026-09-26 the owner
chose "Node card admits work".

A node is a card:

```jsonc
{ "name": "laptop", "always_on": false, "power": "battery",
  "slots": 8, "capacity": { "cpus": 10, "mem_gb": 32 },
  "isolation": ["worktree", "container"], "price_per_hour": 0 }
```

- **Day-one job: admission.** Every live root and kernel on a machine takes a slot. When
  the slots are full, a new root waits in `Pending`.

  This is the bound the family caps don't give: a root is a session plus a kernel, and a
  thousand roots on one laptop is a thousand kernels. The existing width
  `clamp(cores−1,1,8)` becomes that machine's default slot count.
- **One computer is a hive of one.** The card is computed locally, and nothing else
  changes for a single-machine install.
- **Placement across machines is not built** (§8.1).
- **Sleep.** A laptop card has `always_on: false`, so a scheduled tick can fall while the
  lid is shut. The catch-up rule is data on the clock subscription (§5.2). The default is
  to fire once on wake, the way anacron does.

### 3.5 Channel: how outside data moves in

A channel is a named, durable, append-only **buffer** with one home node. The home
assigns offsets, so order is total within a channel and undefined across channels. It
has:
- a **retention** policy, which is required;
- optional **key compaction**;
- messages of at most 16 KiB, with anything larger passed by `store://` reference.

**Authority (law 4).** A channel carries data that has no other home: CI events, alarms,
mail, clock ticks. When a subscription delivers a message, the delivery is appended to the
target's log: the session JSONL through D230's queue (`Queued { message, wakes, news }`,
`crates/runtime/src/session/run.rs:20`), or the plan journal when it unblocks a todo. From
then on, that log is the authority. The buffer may be truncated once delivery is acked,
because nothing reads it for history.

A **subscription** has a filter, batching (`batch {size, window}`), a `min_interval` and a
durable offset. A match has **two** possible targets:
- **an agent:** the message enters its run queue;
- **a todo:** a todo blocked on the channel is unblocked, or a new todo is created.

Starting a saved definition is not a target (law 6).

**Adapters** are the only way in or out. Each is a supervised subprocess that speaks JSON
lines on stdin and stdout, chosen by URI scheme. A source is acknowledged only after the
home has written the message to disk. The only built-in adapter is `clock`.

**Delivery** is at-least-once and idempotent by message id ✚. A channel whose home is
unreachable pauses, and its subscribers see that as a `Blocked` todo, not as silence. That
mapping is ✚ and ⏸ until a channel home can be remote.

**Consumer groups** are not a channel feature. Competing consumers need the claim protocol
(§9.1), which is a new mechanism, ⏸.

### 3.6 Lease

`Lease` today is `holder, parent, deadline_ms, tokens, granted_at, revoked`
(`crates/types/src/lease.rs:10`) ✓. It is drawn from the parent's lease at spawn, only
ever smaller, and unspent budget returns at reap. `parent_close` (`Terminate` or
`RequestCancel`, `lease.rs:80`) ✓ matches Temporal's parent-close policy. The runtime's
`expire` (`crates/runtime/src/lease.rs:310`) repossesses revoked children after their
grace.

A lease has no scope, and it is not a fence. ✚ additions:
- dollars;
- a verification share per intake class;
- model quota.

An exhausted lease suspends a session rather than killing it (✚).

### 3.7 Wall

`deny_write`, `deny_read` and `deny_url` ✓, inherited downward, only narrowing.

**What each one covers**, per `Wall::check_url` (`crates/runtime/src/wall.rs:77-91`):
- `deny_read` walls `local://`, `checkpoint://` and `tree://` paths.
- `kernel://`, `family://` and the other shared schemes are walled only by `deny_url`
  prefixes.

So an exam's held-out probes (§7.1) are hidden with both: `deny_read` on the exam file, and
`deny_url` on `kernel://<grader>` and `family://<exam>`. No new mechanism is needed, but the
right field has to be used.

✚ `deny_channel` walls channel names.

### 3.8 The composite: Plan

A plan is todos plus `after` edges, plus the recorded program that declared them, plus the
journal. It is used for non-trivial work only. Each piece exists ✓:
- `Plan.create(goal, request_id=…)`: a re-run cell opens the same plan, as a Temporal
  workflow ID does;
- `todo(key, …)`: a changed declaration raises `SpecDrift`
  (`python/yi_runtime/src/yi/plan.py:70`), in the role of Temporal's non-determinism error;
- `decompose`, `supersede`, `resume` (which reattaches to state and never runs source);
- the shapes `fork_join` and `scatter`, and the recipe `review_pod`;
- the procedural graph, `crates/runtime/src/prompts/graph.json`.

## 4. Composition

Each row is marked by what it takes: ✓ composes from what exists, ✚ needs a new piece
(named), ⏸ deferred.

| capability | composed from | |
| --- | --- | --- |
| one-off task | a todo (trivial) or a plan (non-trivial), from your prompt | ✓ (two types for now) |
| approval | todo `Blocked { on: User }` | ✓ |
| child agent | a delegated todo in a worktree | ✓; other isolations ✚ |
| review pod, scatter, fork/join | plan shapes | ✓ |
| mailbox | D230's queue + session JSONL + `rlm.receive` | ✓ |
| search | `history.grep` over one session | ✓; `find` over every scheme ✚ |
| outward-action log | a query over the JSONL: every tool call with an outward effect | recipe |
| intent record | addresses of your words on a todo; user text already survives compaction (`floor.rs`) | ✚ the field, ✓ the floor |
| standing preference | the address of your verbatim words | recipe, once the record exists |
| gate | contract + intent judge; the contract has a judge decider (`Decider::Judge`, `contract.rs:226`) | ✓ contract, ✚ intent judge |
| preview | todo blocked on the user with 3–5 options | ✚ options payload |
| cron / heartbeat | `clock` adapter + subscription → a todo per tick | ✚ adapter, subscription table |
| recurring workflow | a tick creates a todo; an agent runs the content-hashed definition | ✚ |
| event-driven work | adapter + subscription → a todo | ✚ adapter, ack-after-disk |
| durable wait / signal | todo `Blocked { on: Channel { filter } }` | ✚ new variant and matcher |
| durable timer | the same variant, aimed at `clock` | ✚ |
| overlap and catch-up | policy data on the clock subscription | ✚ |
| best-of-N | N todos with one intent record, compared by the intent judge | ✚ judge |
| kill switch | a privileged control message the host obeys | ✚ |
| spend alert | a subscription on lease events | ✚ event stream |
| resource admission | node card slots | ✚ |
| discovery | a compacted registry of agent and node cards | ⏸ |
| task queue, merge queue, re-dispatch | the claim protocol (§9.1) | ⏸ new mechanism |

## 5. Work: one shape for every kind

### 5.1 Three independent axes

| | node bodies are code | node bodies are models |
| --- | --- | --- |
| **static graph** | nightly backup and checksum: `run=` todos only, zero tokens | the 9 a.m. triage: fixed steps, model bodies |
| **dynamic graph** | one todo per failing test, computed by code from data | "fix this bug": the model decomposes as it learns |

The third axis is the **trigger**, and any cell can have any trigger:
- a **prompt** makes the work one-off;
- the **clock** makes it recurring;
- a **channel** makes it event-driven.

Every trigger does the same thing: it creates or unblocks a todo. Four things hold in every
cell:
- routing reads data;
- every todo passes its gate;
- the journal records everything;
- recovery reads state.

### 5.2 Temporal, mapped

| Temporal | Yi | |
| --- | --- | --- |
| Workflow ID | `Plan.create(request_id=)` | ✓ |
| Workflow definition | plan program (Python) | ✓ |
| Activity | todo: `run=` code or `delegate=` agent | ✓ |
| Event history | plan journal + session JSONL | ✓ |
| Non-determinism error | `SpecDrift` | ✓ |
| Child workflow + ParentClosePolicy | delegated todos + `parent_close` | ✓ |
| Query / Update | `fetch("plan://…")` / `rlm.request` | ✓ |
| Continue-as-new | `Plan.supersede`, compaction | ✓ |
| Retry policy | `retry` with a cap | ✓ |
| Activity heartbeat timeout | lease deadline + a fenced claim | ⏸ claim protocol |
| Signal | a message that unblocks a todo | ✚ |
| Durable timer | todo blocked on `clock` | ✚ |
| Schedule + overlap policy | clock subscription creating todos; skip / buffer-one / allow as data | ✚ |
| Task queue + workers | the claim protocol | ⏸ |
| Visibility | `find` + registry | ✚ / ⏸ |
| Saga | a failed todo triggers compensating todos | recipe |

**The deliberate differences.**
- **Where a wait lives.** Temporal keeps a waiting workflow's position in code and rebuilds
  it by replay, which requires deterministic workflow code. Yi's plan programs are
  model-written Python, so a wait is a todo blocked on a channel. Recovery reattaches to
  state, and there is no stack to rebuild.
- **What a schedule does.** A Temporal schedule starts workflow code. A Yi tick creates a
  todo, and an agent decides what to run (law 6). A pure-code job therefore costs one
  agent turn per tick. That is the price of never running saved source unattended.

### 5.3 Example: every weekday at 9 a.m.

```python
# the definition lives in the store; its content hash names it
MORNING = "store://sha256:…"            # ✚ workflows/morning.py, frozen at save time

await rlm.subscribe("clock://0 9 * * 1-5", create={          # ✚ a tick creates a todo, nothing else
    "label": "morning triage for {fired_at}",
    "intent": MORNING_INTENT,                                 # your words from setup, by address
    "delegate": {"role": "reader",
                 "note": f"run {MORNING} for {{fired_at}}; its plan uses request_id morning-{{fired_at}}"},
    "overlap": "skip", "catch_up": "once",                    # ✚ policy as data
})
```

- **The run.** The woken agent fetches the frozen definition by hash and runs it. It
  creates one plan per firing, with `request_id=morning-<date>`. Replies are allowed within
  the lease and appear in the outward-action log.
- **Editing the definition** freezes a new hash, and the subscription is updated to point
  at it. Every run's record names the hash it used.

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

The intent record is **a list of addresses** on a todo, and nothing else. It has no text
of its own, so it cannot restate anything. It points at:
- **your messages in the task**: `user://<session>/<n>` or `history://` spans;
- **your answers and picks**, and **your corrections and rejections**. These matter most:
  "don't overengineer", "use my name" and "drop the biology" mark exactly where models
  drift back;
- **exemplars**: the previews you chose (§6.3), as `store://` hashes;
- **standing preferences**: the addresses of your own words.

**Size.** Your messages already survive compaction verbatim within a 64,000-token floor.
Past it, the oldest is middle-truncated (`floor.rs:17-41`), and the full text stays one
fetch away. A paste is material, not intent: it gets an address like anything else.

You asked for "truncation + smart compression for long messages". That lives in how a
record is **shown**: a head and tail window, or a one-line label on a paste. It never
lives in the record. A consumer that needs the words fetches them.

**Where the telephone effect is fixed** ✚. The summarizer's "Goal" and "Constraints &
Preferences" sections (`prompts.rs:9-12`) quote your messages by address instead of
restating them. That is the one place Yi paraphrases you today.

### 6.2 Two-way traceability

Every plan item cites the addresses of the words it serves. Every message in the record is
covered by an item or waived with a reason. Both are mechanical checks, run when the plan
is drafted and after each revision:
- **A message with no item** is something about to be forgotten (step 8).
- **An item that cites nothing** is something nobody asked for (step 3).

A citation is an address plus a byte range, so the check reads your words, not a window of
them.

### 6.3 Previews

"i think always previews, but not heavy previews. previews can be architecture diagrams,
mockups, etc. a pattern i have been recently using to great success is a multiple choice
option for 3 - 5 different styles".

A preview is a todo blocked on the user, with 3–5 options that each carry a light preview
(✚ options payload). What it contains per intake class is set in the table in §7.6.

**Who picks:**
- **you**, when present;
- **the parent**, inside a child. The child's question already surfaces as `needs_you` ✓;
  routing the pick is ✚.
- **the calibrated judge**, when the run is unattended. The losing options are kept, so you
  can overrule it later.

The pick and every rejected option join the record as addresses, and the pick becomes the
**exemplar**.

### 6.4 Boundary checks

At a boundary, the host asks the intent judge your question: "Is this what they meant?
What would they object to first?" The boundaries are:
- the plan is drafted;
- the plan is revised after a discovery (step 7);
- a child finishes;
- done is claimed (step 9, automated).

Which boundaries run the judge is set per intake class in §7.6, one table for both previews
and judges. The trigger is the boundary event, never a model's choice.

### 6.5 The intent judge

- **It reads** the intent record by address, and the exemplars.
- **It sees what you'd see:** the rendered page rather than the code, the slides rather
  than the XML.
- **It compares instead of scoring:** against the exemplar, the previous version or the
  alternatives.
- **It returns a closed verdict enum ✚:**
  - `accept`;
  - `revise { objections, cited_addresses }`;
  - `escalate`, which goes to you, or to the queue when nobody's there.

  The host acts only on the enum. Free-form routing is cut.
- **Calibration ✚ on a held-out split.** Your past reactions in the JSONL (approvals,
  rejections, corrections) are split into a fit set and a held-out set. A judge is chosen
  on the fit set, and it counts only if it also predicts the held-out set. The owner's
  choice: the cheapest judge that passes. Calibration re-runs when the held-out agreement
  drops.
- **Guarding against gaming.** An output that reproduces the exemplar is caught by the blind
  reader probes (§7.2), which test use rather than resemblance. A judge verdict alone never
  accepts work (§7.6).

### 6.6 Taste stays close to your words

Decisions that carry taste (direction, tone, structure, scope) are made by an agent that
reads the intent record directly, on a strong model. Children get bounded mechanical work,
with the record and the exemplar passed by address.

### 6.7 Walkthrough: a landing page

1. **Record:** addresses of your six brainstorm messages: "calm, like Linear but warmer",
   "no gradients", "less copy".
2. **Preview:** five hero mockups. You pick B: "B, but less copy." B is the exemplar.
3. **Plan:** every item cites byte ranges of your words. An animated testimonial carousel
   cites nothing, so it's cut.
4. **Build:** section children get the record and B by address.
5. **Done claimed:** the judge returns `revise`: "They said less copy; features has 140
   words, B had 40", citing your message. The fix is routed before you look.

## 7. Gates

Models are good at generating candidates and bad at judging them. Mechanical checks are good
at judging and can't generate. So a gate makes the model search and lets the host decide.

### 7.1 The exam

At intake, the task becomes probes. Each probe has:
- a question or check;
- the expected answer;
- a decider: command ✓, schema ✓, example ✓, judge ✓ (`Decider`, `contract.rs:226`), blind
  reader ✚ or refuter ✚;
- a weight and a critical flag.

The exam is frozen by content hash (`freeze` ✓, `python/yi_runtime/src/yi/contract.py:23`).
The writer sees the requirements and most of the probes. A held-out share sits behind its
wall (§3.7): `deny_read` on the file, `deny_url` on `kernel://` and `family://`. Every probe
cites the intent address it tests.

### 7.2 Blind consumer tests ✚

A fresh agent does the reader's job using only the rendered artifact:
- answer the deck's probes from the slide images;
- extract the ask and the deadline from an email;
- follow a README in a clean container until the command works.

Answers are compared with the frozen expectations. A failure names its probe.

### 7.3 Refuters search; the host verifies ✚

A finding counts only with evidence the host can check mechanically:
- a quote at a location (`verify_quotes` ✓, `python/yi_runtime/src/yi/roles.py:149`);
- a command that reproduces;
- a number recomputed from its cited source.

### 7.4 Claims carry addresses ✚

Every number, fact and "tests pass" in an output cites an address, which the host fetches and
recomputes.

### 7.5 Probe ownership through decomposition ✚

When a todo splits, every parent probe must be owned by a child or kept by the parent. This
is new. `Contract.covers` ✓ (`crates/types/src/plan/contract.rs:283`) is a different thing:
write-path globs that trigger the checker. It stays the write checker.

### 7.6 Acceptance and one table per class

**Gates and judge must both pass.** Either one failing sends the work back with its reason.
After the retry cap, the work goes to you, or to the queue when nobody's there.

| class | preview | gate | intent judge | example verification share |
| --- | --- | --- | --- | --- |
| trivial | open (§15): "always" vs class-bound | one command | sampled, 1 in N | 5% |
| standard | light: 3–5 options | probes + one blind read | at done | 15% |
| large | light: 3–5 options | + refuters | at every boundary | 25% |
| open-ended | 3–5 options; you confirm the exam when present | + refuters | at every boundary | 35% |

The model sizes the class, and the rules bound it. The shares are data the host enforces.

## 8. Nodes and placement

### 8.1 Isolation

`isolation="worktree"` ✓ grows into a placement URL ✚. `container:rust:1.91` comes first,
on the same machine. Placement on another machine, a VM or a cloud is not built and is
out of scope (YI_DESIGN 1.2); the hive plan is archived in docs/archive/plans/.

### 8.2 Placement drivers ✚

A placement driver is an executable per isolation kind, with two verbs, `up` and `down`.
- **Workspace in:** a git ref plus content-addressed blobs.
- **Result out:** a diff with its gate verdict. `merge_worktree` ✓ generalizes to
  `merge(child)`.

On one machine the transport is a Unix socket.

### 8.3 Reads

Reads are open within one owner by default (D164). Walls narrow them per child, but only
`deny_url` walls `kernel://` and `family://` (§3.7).

## 9. Scale

### 9.1 The claim protocol (⏸, a new mechanism)

Task queues, merge queues and re-dispatch all need the same thing, and none of
it exists:

| piece | why | today |
| --- | --- | --- |
| a single claimer per todo | two workers must not both start it | the plan store is single-writer under `.yi/plans/.lease`; no cross-agent claim |
| an epoch on `Running` | a fencing token | `Running { by }` only |
| a fence on submit | refuse a result whose epoch is stale | not checked; `AttemptId` (`crates/types/src/plan/ledger.rs:113`) counts retries and is not a fence |
| an expiry scan | return an expired claim to `Pending` | `expire` handles revoked children only |
| a group offset | competing consumers on a channel | none |

It gets its own D-row. It is not a composition of todo and channel.

### 9.2 Width

- Width comes from roots. The family fuses stay (law 9), and the node card's slots bound
  roots per machine (§3.4).
- A thousand workers means a thousand kernels, so they only exist with enough machines, and
  enough slots on those machines.
- Model quota is leased (✚), so parallel workers share provider limits instead of all
  hitting rate limits at once.

## 10. Safety on day one

The owner chose three day-one protections:
- **An outward-action log.** A query over the JSONL, so it is a view.
- **A kill switch.** A privileged control message that pauses or cancels every run (✚). It
  is unverified whether today's interrupt already covers a single session.
- **Spend alerts.** A subscription on lease events (✚).

Scopes come after the first overstep.

For channel data:
- filters run before any model sees a message;
- external sources default to batched delivery;
- a wake costs lease;
- messages are rendered with their source, as data.

## 11. Measuring efficiency

- **Token yield:** tokens and dollars of accepted work, divided by the total.
- **Accepted:** the gates and the judge passed, and the JSONL shows no later revert or
  rewrite.
- **Every todo measures itself:** the lease gives cost, the journal gives time, and the
  verdicts plus your later actions give acceptance.
- **Baseline:** the existing session corpus.

## 12. What exists and what's new, on main @ 36cd8eaa

| exists | where |
| --- | --- |
| `rlm.run/service/send/request/followup/wait/receive/result/status/fetch/put/get/ls/revoke/merge_worktree` | `python/yi_runtime/src/rlm/__init__.py` (`receive` :677, kernel objects :950) |
| URL, schemes, durability | `crates/types/src/url.rs:22,160` |
| `family://`, `tree://` | `crates/runtime/src/fetch/schemes.rs:324,356` |
| `Plan`, `todo`, `SpecDrift`, `decompose`, `resume`, contracts, roles, shapes | `python/yi_runtime/src/yi/` (`plan.py:70`, `roles.py:149`, `contract.py:23`) |
| plan `Todo`, `TodoState`, `BlockedOn`, `Delegation.context` | `crates/types/src/plan/doc.rs:74,90,217,234` |
| `Decider::Judge`, `covers` | `crates/types/src/plan/contract.rs:226,283` |
| `AttemptId` | `crates/types/src/plan/ledger.rs:113` |
| session `TodoItem` | `crates/types/src/todo.rs:116` |
| ordered session queue (D230) | `crates/runtime/src/session/run.rs:20` |
| user text kept through compaction | `crates/context/src/floor.rs:5,41` |
| summarizer sections | `crates/context/src/prompts.rs:9-12` |
| `Lease`, `ParentClose`, `expire` | `crates/types/src/lease.rs:10,80`; `crates/runtime/src/lease.rs:310` |
| walls and `check_url` | `crates/runtime/src/wall.rs:77-91` |
| family fuses | `crates/runtime/src/subagent.rs:25-30`; `crates/runtime/src/levers.rs:90` |
| `needs_you` | `crates/runtime/src/family.rs:188` |
| heartbeat scheduler, `should_defer` | `crates/runtime/src/schedule/mod.rs:463` |
| probe ladder | `crates/runtime/src/plan/probe.rs` |
| `history.grep` | `crates/runtime/src/wiring.rs:316` |
| procedural graph | `crates/runtime/src/prompts/graph.json` |

New in this proposal:
- **intent and gates:** the intent-record field; summarizer quoting by address; judge
  replay (stage 0); previews' options; the intent judge and its verdict enum; the exam's
  blind readers, refuters, cited claims and probe ownership;
- **resources:** node cards and admission; `container:` placement and drivers;
- **channels:** the channel buffer, adapters and `clock`; subscriptions creating or
  unblocking todos; `BlockedOn::Channel`; overlap and catch-up policy;
- **safety and search:** the kill switch; spend alerts; `find`.

Deferred (⏸):
- the claim protocol;
- `@node` addressing;
- the registry;
- elastic nodes;
- consumer groups and the merge queue;
- multi-owner intent.

## 13. What this deletes (one in, one out)

- **The heartbeat job store and service.** The cron parsing moves into the `clock` adapter,
  and a tick creates a todo.
- **The probe ladder's polling loop,** which becomes an `exec` adapter plus a todo blocked
  on a channel.
- **Model-written memory notes,** replaced by the addresses of your verbatim words.
- **From the hive plan:** the custom cursor protocol, the idempotency frame field, the
  drop-box ingest, the `WakeCapsule` job and the anchor wake queue.

The mailbox is not deleted: it already is the one ordered queue. The session todo type is
not deleted until stage 0 holds.

## 14. Build order

Each stage has its own demo and gate, and each gets a D-row and an issue when it starts.

| stage | scope | demo | gate |
| --- | --- | --- | --- |
| **0. Judge replay** (read-only; `evals/judge_replay.py`, #609) | A judge replays existing session JSONL. At each recorded boundary it predicts your first objections, citing byte ranges of your messages. It is scored against what you actually said next. Nothing is written back. | a report over the corpus: citation accuracy, and agreement with your later corrections on a held-out split | citations resolve to your words; agreement beats a no-judge baseline. **If this fails, the intent/judge bet is false** and stages 1–2 change before anything irreversible is spent. **Failed on held-out, 2026-09-27 (§14.1).** |
| **1. Your words by address** | the intent-record field on todos (both types); summarizer sections quote by address; two-way traceability | a plan that flags a forgotten message and cuts an item nobody asked for | traceability red on a planted omission; no schema change to existing rows |
| **2. Previews and the judge** | **Reshaped by stage 0: previews only, and the judge routes nothing (#702).** options payload; the intent judge with its verdict enum at the §7.6 boundaries; exam v2; both must pass | the landing page of §6.7 | the judge catches a planted drift; held-out agreement is reported |
| **3. Todo merge** | one type, migration, before/after fixtures | old sessions and plans load unchanged | the schema fixtures before and after |
| **4. Channels** | the `clock` adapter; subscriptions creating or unblocking todos; `BlockedOn::Channel`; overlap and catch-up; kill switch; spend alerts | the 9 a.m. workflow on an awake laptop, and a tick missed during sleep that fires once on wake; a plan that waits for approval at $0 | the eight `mbx-*` trials stay green; the heartbeat store retires |
| **5. Node admission and containers** | the node card for one computer; `container:` placement | eight container children on one laptop, the ninth waiting for a slot | the result and merge path are identical to worktrees |
| **6. Adapters** | `exec`, `file`, `github`, `forgejo` | CI red → todo → verified fix | the probe ladder retires |
| **7. Claim protocol** ⏸ | epochs, fences, expiry (§9.1) | two workers never start one todo | two workers claim one todo in a drive script and the stale epoch's submit is refused |

### 14.1 Stage 0 result, 2026-09-27

Row 0060 of docs/eval-ledger.md records the run. The corpus was 1,248 boundaries, 1,177 of
them from Claude Code transcripts and 71 from Yi sessions, split into fit and held-out by
conversation. Every paid call went to OpenRouter, and the whole stage cost $19.31.

A first labelling pass through `yi ask` was aborted after 99 calls and $0.37: only 30 came
back with a label, because the agent loop kept calling tools past the JSON answer. The replay
then called OpenRouter directly.

The first design (v1) was wrong, not only its judges. Its ground truth was the owner's
attention: a flaw the owner didn't notice was labelled accepted, so a judge that found it
scored as wrong, and finding it is the capability under test. The judge read the agent's own
summary, which is the restatement §1.3 says a checker must not grade. It tested an outside
reader, while the owner's steps 9–10 in §1.3 describe the working agent, with its full
context, being asked to check itself. Its positives mixed intent loss, which rests on words
the owner already said, with new requirements and opinions, which nothing can predict. A
verdict with no score tied every run to one threshold. And the sample, sorted by id, put the
negatives first, so the cap left Opus with 9 positives in 61.

v2 renders the full session prefix up to the boundary and runs two arms over it: `self`, where
the agent is asked the owner's check about its own session, and `judge`, the outside reader.
Both return `p_objection`. A positive is `intent_loss` or `check_revealed`; the 191 `new_info`
objections are left out. Scored on AUROC, the fit split holds 643 boundaries with 101
positives, and held-out holds 404 with 76. The gate asks for a held-out AUROC lower bound above
0.5, citation resolution of at least 0.95, and zero calls without a verdict.

| design | model | split | n | score, 95% interval | citations | catch | cost |
| --- | --- | --- | --- | --- | --- | --- | --- |
| v1, prompt 7b739264b76e | glm-5.3-flash | fit | 200 | balanced 0.540 [0.467, 0.603] | 0.929 | 0.170 | $0.45 |
| v1, prompt 8e00b1b79830 | glm-5.3-flash | fit | 199 | balanced 0.499 [0.438, 0.557] | 0.950 | 0.030 | $0.45 |
| v1, prompt 7b739264b76e | glm-5.3, effort high | fit | 200 | balanced 0.520 [0.459, 0.579] | 0.981 | 0.040 | $1.85 |
| v1, prompt 7b739264b76e | glm-5.3, effort high | held-out | 28 | balanced 0.667 [0.437, 0.833] | 0.982 | 0.000 | $0.21 |
| v1, prompt 7b739264b76e | claude-opus-5.5 | fit | 61 | balanced 0.670 [0.525, 0.872] | 1.000 | not matched | $2.59 |
| v2, `self` | glm-5.3-flash | fit | 93 | AUROC 0.560 [0.432, 0.689] | 0.931 | 0.423 | $1.29 |
| v2, `judge` | glm-5.3-flash | fit | 99 | AUROC 0.654 [0.539, 0.759] | 0.995 | 0.357 | $1.29 |
| v2, `judge` | glm-5.3-flash | held-out | 108 | AUROC 0.577 [0.470, 0.677] | 0.959 | 0.346 | $1.36 |
| v2, `judge` | claude-opus-5.5 | held-out | 18 of 19 | not scored | | | $6.15 |

Labels cost $0.71 for v1 and $2.57 for v2, both from glm-5.3-flash; the v1 matcher cost $0.02.
The held-out run of the `judge` arm on glm-5.3-flash fails the gate twice: its lower bound is
0.470, and 2 calls came back without a verdict. The Opus held-out run stopped when OpenRouter
answered 402, credits exhausted, after 20 calls ($6.15, one boundary refused twice), which is
too few to score.

The limits on this result. The labels come from one small model and were never checked against
the owner. The corpus is mostly Claude Code transcripts: Yi's 17 held-out boundaries hold no
positive. Only one prompt, one arm and one model were scored on held-out; the `self` arm, the
one closest to steps 9–10, ran on fit only. Opus was never scored under v2. So the result says
the intent/judge bet is not supported at this power, not that no judge can work.

Per the stage 0 gate, stage 2 changes before anything irreversible is spent: it keeps the
previews of §6.3, and the judge routes no work (#702). Stage 1 (#690, #701) does not depend on
the judge and stands as written.

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
| first placement | local containers |
| acceptance | "contract passed is bare minimum"; the judge is framed on "the platonic ideal of the extrapolated intent" |
| judges | may route |
| intake depth | the model sizes it, rules bound it |
| verification budget | per intake class |
| hidden probes | a held-out share |
| judge model | the cheapest that passes calibration |
| intent record | all of your words; "truncation + smart compression for long messages" |
| standing preferences | quote plus address |
| previews | "always previews, but not heavy previews" |
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

**Owner decisions, 2026-09-26, on the review:**

| fork | decision |
| --- | --- |
| what "one ledger" means | "One authority per fact" |
| may a clock tick start saved Python | "Clock only creates a todo" |
| first move | "Judge replay first" |
| what bounds roots on one machine | "Node card admits work" |

How revision 2 reconciles the two sets:
- "Judges may route" stands, through the verdict enum.
- The todo merge stands, after stage 0.
- "Partition: wait on the lease, then re-dispatch" stands, and needs the claim protocol.
- "The task's owner wins" is deferred until Yi has an owner principal: today `Lease.holder`
  names a child, not a person.

**Open:**
1. Previews on trivial todos. The owner said "always", and the review would bind previews to
   intake classes.
2. Whether background todos wait for AC power on battery.
3. Whether the missed-tick default stays "once on wake" for every schedule.

**D-rows owed before code:**
- law 4 as "one authority per fact", with channels as buffers;
- channels, reversing the OS plan's cut of pub/sub topics and cross-root messaging,
  "staged, not rejected";
- the clock creating todos only (law 6);
- judges routing through the verdict enum;
- the intent-record field and the summarizer rule;
- node cards and admission;
- previews' options payload;
- the todo merge, with migration and fixtures;
- the claim protocol, when stage 7 starts;
- the YI_DESIGN §1.1 edit for one-in-one-out.

## 16. Not building

- promotion of repeated work into workflows ("overengineering for now");
- Temporal-style replay of saved source, and clock-started definitions;
- a channel as authority for anything already delivered;
- free-form judge routing;
- `deny_read` as the only probe hide;
- `should_defer` as an overlap policy, since it gates heartbeats only
  (`schedule/mod.rs:463-466`);
- a workflow engine, flow language or cluster scheduler;
- CloudEvents as the internal format;
- permission scopes, until the log shows the first overstep;
- new model tools: everything lives on `rlm` and `yi`.

## 17. Review, 2026-09-26

An adversarial review checked revision 1 against main at `36cd8eaa`. Every code citation it
made was re-opened for this revision.

| finding | verdict | change |
| --- | --- | --- |
| channels break "one ledger"; plans already journal separately | held | law 4 is "one authority per fact"; a channel is a buffer |
| held-out probes leak through `kernel://` and `family://` | held | walled with `deny_url` as well as `deny_read` (§3.7) |
| clock-started definitions run saved source | held | a tick only creates a todo (law 6, §5.3) |
| `should_defer` is not an overlap policy | held | overlap and catch-up are new data on the subscription |
| lease expiry does not re-dispatch; `AttemptId` is no fence | held | the claim protocol is named as new and deferred (§9.1) |
| a laptop that sleeps cannot fire a clock | partly | demoable while awake; the missed-tick rule is data, defaulting to once on wake |
| always-previews and a sampled judge are two policies | partly | both are owner decisions; one table per class (§7.6); trivial previews left open |
| the intent record's summaries are restatements | held | the record holds addresses only; windows are display |
| compaction's verbatim floor already exists | held | marked ✓; the fix moves to the summarizer's sections |
| `covers` is write globs, not probe ownership | held | probe ownership is ✚; `covers` stays the write checker |
| merging todo types first is the most irreversible first step | held | stage 0 (judge replay) first; the merge is stage 3 |
| Node should merge into a card on a channel | rebutted | the review's own fix (bound roots by slots) makes the node the admission authority; its placement role is deferred |
| queues are not a composition | held | claim protocol ⏸ |
| cross-node reads are open while a lease has no scope | held | stated plainly in law 7 |
| depth default is 1, not 3 | held | law 9 |
| `@node` is not today's URL grammar | held | marked ⏸ |
| the inventory was grounded on a branch | held | regrounded on main @ 36cd8eaa |
| two owners have no principal | held | deferred |
| judge calibration can overfit history; an exemplar can be copied | held | held-out calibration; blind-reader probes test use, and a judge alone never accepts |
