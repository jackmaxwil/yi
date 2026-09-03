# Yi, the proposal: four primitives and a plan engine

```
status:  PROPOSAL. Self-contained capstone of the 2026-08-31 design sessions.
         Where it conflicts with earlier session docs, this document wins.
         The companion docs (fan-out, ARC pipeline, architecture) and the
         YI_DESIGN §8.9.1 kernel addendum live on the design branch
         `claude/yi-fan-out-t4p302`, not yet on main. Nothing here is
         scheduled; landing requires a D-row in ARCHITECTURE.md per the
         house workflow.
date:    2026-08-31
inputs:  benchmarks read from source (terminal-bench v4.0.0, arc_agi 0.9.1,
         arcengine 0.9.3, harbor, SWE-Atlas) · a Pi fork (todo.ts,
         todo-tracker.ts, task prompts) · the reference harness
         main@9f5edc1 + branches (rlm/repl.md, SwarmRolePolicy, rlm-ledger)
         · arXiv 2604.11378 (SGH), 2311.05772 (ADaPT), 2312.04511
         (LLMCompiler), 2502.14563, 2510.25320 · Yi ground truth:
         YI_DESIGN G2/G5, B-series, K10, D25, D58, E4; .ruler/skills/har*
note:    twenty-eight scenarios, three pressure rounds, and a final review
         shaped this; §11 records what was proposed and killed on the way.
```

## 1. What this is

One configuration of Yi that succeeds at normal coding and, unchanged, at
Terminal-Bench 4 and ARC-AGI-3. Nothing in the tree keys on a benchmark's
identity: Yi reads the task, measures the machine, and diagnoses the regime
at runtime. The design is four primitives and one new subsystem — the plan
engine — built from five newtyped ids, three structs, two state machines,
and one op vocabulary.

## 2. Four primitives

1. **A persistent, fast kernel as working memory.** The IPython kernel is
   where a run's *knowledge* lives as data: the map of the codebase, the
   hypotheses about a game's rules, what was tried and failed, every
   collected child result. Context windows compact; the kernel does not
   (K10 snapshots per variable). A run that keeps knowledge in prose gets
   lobotomized at every compaction; a run that keeps it in kernel variables
   does not.
2. **Adaptive decomposition — the engine.** At every moment, know the next
   smallest unit of work, cut so its outcome is observable, with exactly
   the context it needs. It is a loop, not a plan: decompose → attempt →
   observe → re-cut. Ordering is where "deliverable exists early" actually
   lives — skeleton first, end to end, then deepen — a cut rule, not a
   virtue. Fan-out is the special case where units verify in isolation.
   §3–§6 are this primitive made concrete.
3. **Expectation before action.** Every action carries a prediction; the
   observation either confirms or surprises, and surprise is the only
   honest trigger for rethinking. A test is an expectation about behavior
   and nothing more — a green check is an unsurprising probe, never a
   finish line; in ARC the "check" is prediction vs frame, no command
   exists. Strong evidence is a sustained record of correct predictions
   across diverse probes.
4. **The topology is the stuck-prevention.** There is no sweep and no
   unsticker. Main owns the ledger, the git-persisted plans, merges,
   commits, pushes, and the user; children execute acceptance-criteria'd
   leaf tasks too small to drift, and recon runs on cheap read-only
   scouts. The only drift left is main's own ledger diverging from main's
   own work — detected by events (mutations without ledger touches),
   never by schedule.

## 3. The shapes

The ontology is `GOAL → PLAN → TODO → (PLAN → TODO)`. GOAL is the why —
already a session fact (G2, D25) that no message can redirect; it survives
every replan. PLAN is one version of the how. TODO is the atom. TODO and
PLAN alternate: a plan contains todos; a todo may open at most one
sub-plan. The critical split, learned from the reference: the **ledger row is soft**
(labels, discovered and edited constantly) and the **hand-off is hard**
(acceptance mandatory, because a child is about to run blind on it). They
are different objects.

```rust
// house derive line on every id
GoalId(u64)  PlanId(u64)  TodoId(u64)  PlanVersion(u32)  RetryCount(u8)

Plan {
    id: PlanId,
    goal: GoalId,              // replans never touch it
    version: PlanVersion,      // bumped by supersede only, never by edits
    tier: PlanTier,            // Root | Sub — the whole depth rule
    todos: Vec<Todo>,          // Vec order IS priority; reorder is an op
    state: PlanState,
}

Todo {
    id: TodoId,
    label: TodoLabel,          // verbatim-content identity, unique per plan;
                               // models address by label, never by id
    after: Vec<TodoId>,        // ordering edges: sibling-only, acyclic,
                               // checked at insert. Data does NOT ride edges
    state: TodoState,
    delegation: Option<Delegation>,   // None = main works it inline
    subplan: Option<PlanId>,   // set once, via decompose(); setting it IS
                               // the parent notification (a lane event)
    retries: RetryCount,       // saturating, capped
}

Delegation {                   // the hard object — rigid on purpose
    spec: SpawnSpec,           // role, model, effort, tools, isolation,
                               // budget slice (allocation, never minting)
    accept: Check,             // MANDATORY: Command(cmd) | Stated(text)
    context: Vec<Url>,         // addressable references (§3.2), plus one
                               // size-capped inline note
    output: Option<SchemaRef>, // declared ⇒ Done requires validated output
}

TodoState { Pending, Ready, Running { by: AgentId },   // host-internal id;
                                       // the address is the todo label
            Blocked { on: Child(AgentId) | User | External, note },
            Done { output: Option<OutputRef> },   // a URL (§3.2)
            Failed { cause }, Abandoned }

PlanState { Active, Done, Superseded { by: PlanVersion }, Abandoned }
```

Acceptance is a typed envelope around prose or a command: the runtime
checks existence, size, and provenance; the content is for models. There is
no contract field anywhere — shared context is a document like any other,
referenced by URL from the delegations that need it. `Done`
carrying the only output field makes "completed without an output" and
"output before completion" unrepresentable where a schema was declared —
enforced in the step table, because whether an output is required is a
delegation property, not a todo property (ARC's "reach level 2" completes
bare).

### 3.1 Where the strings live

Plans are files: one Markdown document per plan, JSON frontmatter (D105), in
`.yi/plans/` at the workspace root — tracked by git, one config key
(`plans.dir`) to move or ignore it. Not `docs/plans/`: that namespace is
for human-authored deliverables, and a machine-churned ledger would
pollute it — main *promotes* a plan's prose there when it is worth
keeping, which is authorship, not storage. The frontmatter is
the machine truth — ids, version, tier, todo states, edges, delegation
specs, refs — small, diffable, hand-editable. The body is prose keyed by
todo label — acceptance detail, notes, shared context worth keeping beside
the plan — and every section is addressable (§3.2), so a delegation cites
what it needs by URL instead of a plan carrying a contract field. GitHub
issues are the outward mirror: the
reconciler (fan-out §13) renders the frontmatter as a task list into the
issue body and routes inbound comments through the yard as proposals.

What makes files safe as the canonical store:

- **One process writes.** The host applies every op with an atomic
  tmp+replace; per-plan ownership is enforced at the op seam, not with
  file locks. A torn write cannot exist; a crash loses at most one op.
- **The user's editor is the sanctioned second writer.** The host
  re-reads on mtime change and at turn start; a user edit diffs into ops
  attributed to the user. Reorder, add, check off — in plain text.
- **History is free twice**: the shadow gitdir (T14) captures every plan
  change without a commit, and main commits plan files at milestones like
  any other deliverable, so plans survive machine loss by push.
- **Big content never enters the file.** Everything referenceable is an
  addressable URL (§3.2) — kernel variables, workspace files, plan nodes,
  live agents; the plan holds labels, capped prose, and URLs — a 40-todo
  plan is a few KiB.
- **The kernel is never the store**: K10 skips unpicklables silently and
  prunes oversized variables, and the plan must not depend on dill.
  References point from plan to kernel, never back.
- A child in a worktree writes its sub-plan file in its own checkout; it
  merges at hand-back like any other work product, and live inspection
  reads it in place through the family registry.

The transcript still records every op as the tool call it was — audit for
free — but the file is canonical: rehydration, the TUI tree, inspection
from any agent, and the issue mirror all read the same document the user
can open in an editor.

### 3.2 Settled questions

**Everything is an addressable URL — everything.** One reference type,
one read path, an **open** scheme set behind one resolver seam. Internal
schemes:

| scheme | resolves to |
|---|---|
| `local://<path>` | workspace files and artifact spill files (T16's spill ids fold in: recovery references are `local://` URLs, not bare ids) |
| `kernel://<agent>/<var>` | a variable in any agent's kernel namespace; owner elided = the plan owner's. B14 result collection, the tasking bus, the post-compaction peek, and live ipython inspection are all this one fetch |
| `plan://<plan>[/<todo label>]` | a plan, or one todo's state + prose + output |
| `agent://<plan>/<todo label>` | a live delegation's trace and current output. Children have no names: they are addressed by the todo they execute — the plan is the roster, and a second naming scheme is a shadow model of the first. `agent://main` is the root |
| `history://<agent>[/<entry>]` | transcripts and entries — a cited decision, a compaction summary, the advisor's digest ids (the reference precedent) |
| `checkpoint://<tree>/<path>` | a file as it was at a shadow-gitdir capture (T14); last-good plan recovery is a fetch of this |
| `mcp://<server>/<resource>` | MCP resources — URI-addressed in the protocol already, fetched through the one-shot CLI |
| `user://<n>` | the n-th user message, verbatim — resolves only for entries with genuine user attribution, so a fetched `user://` is unforgeable authority |

— and the set is not closed: `https://`, `github://` (an issue, a PR, a
file at a ref), `s3://`, a Docker or sandbox/VM `mount://` — any resource
a resolver can fetch is a legal reference in a delegation context, an
output, or a plan body. What varies by scheme is not the type but the
terms, and the resolver seam enforces all three: **permission** (every
fetch passes the broker, and B15 walls become URL-prefix denies — one
vocabulary across files, kernels, agents, and remotes; an external scheme
is a tool call, not a free read), **provenance** (content from outside
the trust boundary arrives yard-fenced, E4), and **cost** (internal
schemes are free; remote fetches are priced like any other I/O and cached
by the resolver). Delegation context is a list of URLs plus one
size-capped inline note; `OutputRef` is a URL. Inspection from user,
parent, child, or sibling is the same fetch — pull is on-demand and cheap
where it is local; push is always budgeted.

**References carry expectations.** `local://` and `checkpoint://` URLs
take hashline fragments — `local://src/auth.rs#L42-58@a3f2`, T6's xxh32
tag as the anchor — and a fetch whose tag no longer matches fails loud.
Primitive 3 applied to context itself: staleness becomes a surprise event
at fetch time, never silent rot under a working child.

**Fetches pin.** The resolver's cache already hashes what it serves; a
dispatch records the hash of everything its child fetched, and `retry`
may pin those hashes to replay identical context. A pinned re-fetch that
diverges says *the world moved*, not *the child failed* — the staleness
signal recovered from the cache log for free.

**References are for action or for evidence, and the seam knows the
difference.** An action reference — context handed to an agent about to
work — is live and tagged, and *wants* to fail loud when stale. Evidence
— a claim's citations, a `Done` output, a `FailCause` — is pinned at the
moment of record: `checkpoint://` for workspace files, `@sha` for git
objects, the fetch-hash for everything else. Three rules, one mechanism:

- **Schemes are ephemeral or durable; terminal records are durable-only.**
  `agent://` and `kernel://` are legal in live coordination and illegal
  in anything that outlives its referent. At record time the host
  downgrades: a reaped child's trace is already `history://`, a child
  variable is the promoted `kernel://main/…` or a spilled `local://`, a
  touched file is `checkpoint://`. Enforced in the step table — a
  terminal record carrying an ephemeral URL is unrepresentable.
- **Pins are minted by the system, never by agents.** An agent cites only
  what it fetched, in the live form it fetched it. The resolver's log
  already holds the tree, tag, and hash of everything it served; the
  host rewrites citations to their pinned forms at the terminal seam. No
  model ever produces or echoes a hash — the reference id-lesson applied to
  cryptography.
- **A citation absent from the fetch log is an unbacked claim.** The
  child cited something it never read; the lookup fails and the claim is
  flagged mechanically — YI_DESIGN §7.4's fabrication detection given its
  mechanism, at zero model cost. Pins are guaranteed within-session;
  cross-session evidence is re-verified before it is trusted, which is
  the right epistemics regardless.

**User words are durable, unforgeable authority.** `user://<n>` serves a
user message verbatim, and only genuinely user-attributed entries
resolve — an agent cannot mint one. User messages stay reachable by URL
through every compaction (the entry tree is append-only; the URL is the
recall path), and ops gated on user authority — the goal edit, D25 at
the op seam — get their mechanism: cite the `user://` that authorizes
them, verified at resolve.

**`fetch(url)` is the runtime's one data builtin.** Typed by scheme —
`kernel://` returns the live object, everything else bytes or text — and
batching is `asyncio.gather`, no API. One function is the entire
kernel-side data surface.

Four things fall out of the one seam that no single scheme justifies:

- **M3 context-relevance telemetry is free.** *Supplied* is the URL list
  in a delegation; *referenced* is the resolver's fetch log. Over- and
  under-supply become a grep, uniform across every medium — the fan-out
  doc's most valuable metric with its instrumentation cost deleted.
- **One enforcement point.** Permission, provenance, and cost are checked
  once, at the seam, for every scheme — no per-scheme security model can
  drift.
- **Cache-friendly context.** A URL in a prompt is a short stable string;
  content is fetched when needed. Heavy context stops moving transcript
  bytes, composing with the E7 cache breakpoints instead of fighting them.
- **The corpus is addressable.** `history://` spans sessions, so
  learning-across-runs (fan-out §12) and session mining read one address
  space — and a fitted constant can record its provenance as the URLs it
  was learned from, evidence trail included.

The user surface is the same seam: `yi fetch <url>` at the CLI, and the
TUI opens any URL.

**What deliberately does not fold** — or the paradigm rots: no verbs
(`exec://`, `spawn://` — URLs name nouns; effects stay tools under
permission); no writes (URLs are read-only; every write remains an op or
a tool call, or single-writer and the broker are dead); no query
languages (slicing a frame window is the kernel's job — the URL names a
variable, Python computes the view); no telemetry-lane scheme (offline
consumers read files); no secret scheme, ever (the broker injects
credentials; a URL naming one is unrepresentable); and resolvers are
built-in only — new schemes arrive as Yi releases, never as
extension-registered code.

**The frontmatter, `format: 1`** — the machine truth, schema-validated on
every read, last-good recovered from the shadow gitdir:

```json
{
  "format": 1,
  "plan": "7f3a-auth-refactor",
  "goal": "Ship OAuth login end to end",
  "version": 3,
  "tier": "root",
  "state": "active",
  "todos": [
    {"label": "Freeze the token API seam", "state": "done",
     "output": "kernel://token_api_seam"},
    {"label": "Implement refresh flow", "state": "running",
     "after": ["Freeze the token API seam"],
     "delegation": {
       "spec": {"role": "coder", "effort": "med", "isolation": "worktree"},
       "accept": {"command": "cargo test -p yi-ai refresh"},
       "output": {"schema": "local://.yi/schemas/refresh_result.json"},
       "context": ["plan://7f3a-auth-refactor/seam-notes", "local://docs/auth.md"]
     },
     "retries": 1}
  ]
}
```

Editing a goal is a user-attributed op (D25); `tier: sub` adds `parent:
<plan>/<todo label>`; ready is never stored, it is derived from `after`
plus states at read; the running child of a delegated todo is
`agent://<plan>/<slug>`; a label is at most 80 chars, unique, and the
address. Comments do not survive a rewrite, so the file carries none.

Blocked stores its discriminant inline (`blocked: {on: user, note: …}`).
Frontmatter ≤ 32 KiB. Body: one `## <todo label>` section per todo that
needs prose.

**The tool.** One `plan` tool, `op` parameter, the reference's shape. Addressing by
verbatim label. `view` returns the windowed frontier; `view: full` the
whole tree. The owner gets mutating ops; anyone else is refused with
"propose to the owner" — proposals travel as messages (B6), never through
the tool. Depth-2 agents get `view` only. Ops always batch with real work.

**Dispatch is a respectful queue into the owner's session.** When the
ready set grows — an edge satisfied, a child reaped — the host enqueues a
follow-up into the owner's session queue naming the newly Ready todos:
delivered at a message boundary when the owner is mid-turn, triggering a
turn when idle (Pi's steer/follow-up queues; B13's semantics). Never an
interrupt, never a poll; the owner dispatches the batch in one call.
Auto-dispatch is a config-gated optimization for after the fixtures prove
the manual path.

**And the rest, one line each:**

- Goal text is denormalized into the root frontmatter; the op editing it
  accepts only user-attributed writes — D25 at the op seam.
- Issues: one per **root** plan, sub-plans as nested sections; mirror
  **off by default**, one-way render out, inbound comments as yard-fenced
  proposals. TB4/ARC degrade to files-only. The `plans.mirror` key lands
  with the mirror: a config switch for a feature nobody wrote reads as a
  capability the tree does not have, so it is not carried ahead of it.
- Concurrent sessions: `.yi/plans/.lease`, K1's lock discipline verbatim
  (pid file; stale = pid dead ∨ mtime > 30 s); `view` works, mutation is
  refused with the lease named; stale takeover is a recorded event.
- Lifecycle: one file per plan identity, version inside; supersede
  rewrites in place, history in the shadow gitdir plus milestone commits;
  Done plans stay — kilobytes, and the corpus offline fitting mines.
- `Blocked{on: External}` may carry an optional probe (`Check::Command`,
  reused); the host runs it on a saturating ladder (60 s doubling, cap
  30 min) and a pass becomes a host-attributed `unblock`. No probe → nudge
  the owner at the cap interval.
- Loop constants (nudge threshold, nudges/cycle, reminder budget) inherit
  the reference's shipped values, named once, refit offline on task-shape features —
  never per benchmark.
- Multi-root workspaces: the primary root owns `.yi/plans/`.
- Depth-2 visibility: pull the full tree freely; the pushed context is
  lineage only.

### 3.3 Reconciliation with the landed plan module (0.33.0)

YI_DESIGN §8.17.1 shipped a plan primitive before this proposal:
`Fact::Plan` beside `Fact::Goal`, legality-table transitions, derived
`frontier()`, host-verified done, expand-only `plan.edit` (D53's
`SHRINK_ERROR`), a 12-turn latched reminder. Affirmed superseded where it
conflicts, carried forward where it agrees:

- **Storage is superseded**: `Fact::Plan` becomes a *pointer* to the
  canonical `.yi/plans/` file — compaction-immunity preserved, content in
  the file (§3.1).
- **D53's expand-only rule is reconciled, not repealed**: `drop` marks
  `Abandoned` — a state, nothing deleted; rewording is append-new plus
  abandon-old; `supersede` is the audited exception D53 was defending
  against silent versions of — version+1, the old cut preserved in git
  history. The anti-laundering intent survives; the mechanism generalizes.
- **Carried forward unchanged**: D26 ("no advisory-only state tool" — a
  plan tool is checked at turn end or it is a print statement with a
  schema) and host-verified done — the same instinct as the delegation
  `Check`, arrived at independently in the codebase.

## 4. The ops and the two granularities

the reference's vocabulary, extended for the DAG:

| op | effect |
|---|---|
| `init` | create the plan; forced as the first tool call on multi-step work |
| `append` / `drop` / `block` / `unblock` / `reorder` / `add_edge` | in-place edits to the Active plan; **no version bump** |
| `start` / `done` / `fail` | state steps, via the one transition table |
| `retry` | Failed → Ready; replaces the Delegation (bigger model, new context carrying the failure cause); resume rides the child's K10 snapshot, keyed by todo; may pin the prior dispatch's fetch hashes to replay identical context |
| `decompose` | open a sub-plan on a Running todo; Root tier only, else `DepthExhausted` |
| `supersede` | replace the whole cut, version+1 — the only structural rewrite |
| `view` | echo; models re-read labels instead of guessing |

Two granularities on purpose: cheap in-place ops for the constant churn of
discovery, `supersede` reserved for re-thinking the cut. Versions audit
re-cuts, not bookkeeping. Scheduling is deterministic: the ready set is
computed from edges (all `after` satisfied), dispatch takes the whole
ready set (LLMCompiler's result: up to 3.7× latency from exactly this),
and the earliest Ready in Vec order is the pointer. The model decides
content; topology decides eligibility.

## 5. The rules

- **Single writer per plan**: the agent that opened it. Children and
  siblings *propose* (a message); the owner disposes. User commands route
  through main; the actor rides the event, not the struct.
- **Supersede cascades**: `Active → Superseded` is legal only after
  Running delegations are interrupted and reaped. Zombie children are
  unrepresentable, not discouraged.
- **Rehydration**: on resume, `Running` whose agent no longer exists
  reconciles to `Ready`, retries intact.
- **Output lifetime**: promotion runs at **reap, whatever the outcome** —
  collect is a reap-time action, not a success-time one. A todo's output is
  promoted into the plan owner's kernel (B14's schema seam), and a child
  that failed after doing real work carries its last product as
  `Failed{cause, last}`, host-minted at reap like every other pin, so
  `retry`'s new context can *fetch* the failed attempt instead of being told
  about it in prose. The terminal-record rule needs no extension to cover
  it: `Failed` is terminal, so `last` is `history://`, `local://` or
  `checkpoint://` and never `agent://`. Sub-plan intermediates die with
  their child unless folded into the todo's output. Live children are
  inspectable at will; dead ones through what was promoted.
- **Windowed injection**: continuations and compaction re-inject counts
  plus the frontier (Ready/Running/Blocked, next few), never the full
  ledger. (Scoped-context replanning measured at −82 % tokens, TDP.)
- **Unit of decision, not iteration**: 300 mechanical renames are one todo
  with a kernel loop inside, not 300 todos. ARC's 30 probes/minute never
  touch the ledger; todos sit at level grain. The ledger is slow thinking;
  the kernel is fast thinking.
- **No budget minting**: a sub-plan has no budget field; a delegation's
  slice is an allocation of the parent todo's own allowance.
- **A spawn ceiling is a fuse, not a budget**: `Spawns` rides the *root*
  plan's frontmatter — sub-plans charge the root — is charged at one site
  on every dispatch path, and is monotonic across `supersede` and
  rehydration both, because a counter that reset is the runaway it exists
  to catch and an in-memory one makes crash-resume unbounded. A budget
  divides a real resource fairly; a fuse catches a bug in the model's own
  control flow and should never fire in a healthy run. The user may edit
  it down in the file, as the sanctioned second writer.
- **Width is measured, never configured**: the dispatcher admits
  `clamp(cores − 1, 1, 8)` children — `available_parallelism`, reserving
  main's own kernel and builds, clamped *low* as well as high, because a
  2-core container is exactly where `cpus − 2` goes to zero. The queued
  follow-up names only that slice, so backpressure needs no state: nothing
  is marked `Running` before its child exists, and `start` is never
  refused for width. `bash()` handles are not children and never charge
  it — conflating them would defeat the point of preferring them.
- **A cap is refused or reported, never silently applied**: the 32 KiB
  frontmatter cap is checked *before* the write and refused with what
  exceeded and by how much, never written-then-trimmed; a windowed `view`
  counts what it hid as well as what it showed; a held-back ready todo is
  named in the follow-up, or backpressure reads as an empty ready set.
  There is no separate todo-count cap: the byte cap binds first, and a
  second one would be dead code beneath it.
- **Authority does not recurse**: merge, commit, push, and user
  communication have no op on Plan or Todo at all; they exist only on
  main's session, whoever decomposed.

## 6. Coupling to the loop (the reference's proven mechanics, kept)

- **Eager init**, forced via `tool_choice` on multi-step work; skipped
  when the prompt is a question. Cover the whole request, investigation
  through verification; every user-enumerated item its own todo.
- **Never a solo ledger call** — ops batch with real work in the turn.
- **Work-triggered nudge**: N mutating tool calls without a ledger touch
  (the reference ships 12) fires one hidden reconciliation nudge, at most twice per
  prompt cycle. Divergence is an event, not a timer.
- **Stop interception**: a terminal turn with open todos gets a bounded
  reminder and a forced continuation — suppressed when genuinely asking
  the user, when async work will re-wake the loop, and for `Blocked`,
  whose `on` discriminant decides posture (child: keep working; user: end
  the turn and ask; external: check on a cadence).
- **The plan is the compaction spine**: rehydrated from its file (§3.1),
  re-injected windowed, with the reference's staleness protocol — before substantial
  work, compare the next action against the frontier; fix the ledger
  first.
- The TUI tree and statusline render the ledger directly (U20 stays a
  view); the full DAG — every plan, todo, state, edge — is readable by the
  user and by any agent at any depth.

## 7. The kernel underneath

- **IPython, long-term.** the reference v0.9.0 replaced its Jupyter kernel
  with a stdio CPython REPL (2.7× per command, 14× boot); YI_DESIGN §8.9.1
  records Yi's investigation and rejection. The numbers do not price Yi's
  loop: it is model-bound, inner loops run *inside* the kernel (one
  execute, many actions), kernels boot lazily so a child that never
  touches one never pays one, and a single warm kernel makes the first
  boot perceived-zero — while IPython keeps giving free what the reference now
  hand-maintains (interrupts, per-cell output attribution, fd hygiene).
  The async `bash()` handle API (`h = bash(cmd)`; `h.tail/poll/kill`;
  `await h`) runs on the IPython kernel as it stands and is the
  latency-hiding tool of first resort on 2-core machines — overlap a build
  with thinking before ever spawning a child.
- **K10's contract is the durability law** for everything the primitives
  store: kernel state must pickle clean (unpicklable is skipped silently)
  and stay under the per-variable cap (16 MiB; post-compaction prune).
  Plan outputs, world-model state, collected results — small state, code
  as code.
- Parent→child tasking, results, messaging, and inspection all ride the
  existing seams: spawn (B5), messages (B6/B13), schema-checked results
  (B14), worktrees (B11), walls (B15), permission inheritance (B10).

## 8. What the benchmarks taught (verified from source, then generalized)

| fact, verified | consequence in the design |
|---|---|
| TB4: all 66 verifiers floor to reward ∈ {0,1}; the verifier runs later, in another container, seeing only declared artifact paths | self-check hard (`Check` at collect), aim past stated thresholds, create the declared artifact early — it kills the undeclared-path zero, not a partial-credit myth |
| TB4: 8 h stated in every instruction; 2 CPUs on 44 of 66 tasks | budgets are read from the task, never configured; fan-out there is latency-hiding — prefer `bash()` handles to children |
| ARC: score = baseline/actions per level, capped 100; unreached level = 0; mean over levels; `FrameData` carries no action counter | depth beats elegance; wrong actions are the cost, so predict before acting (primitive 3); count your own actions |
| coding: no hard budget exists | the standing trade-off — speed, stability, accuracy, token cost; `Budget.total` is optional and absent by default |

The regimes differ in exactly two things — the cost of checking and the
parallelism the environment allows — and both are observable at runtime.
That observation replaced every benchmark-specific setting this design
ever considered.

## 9. Recovery, bounded

On `Failed`, the owner's ladder is **retry → decompose → supersede**:
same cut with a stronger executor; then split the todo (ADaPT: decompose
*on failure*, not upfront — worth up to +33 points over fixed cuts); then
admit the cut was wrong. Each rung is bounded (`RetryCount` caps, depth
caps, supersede gated on reaping) — the failure-loop pathology that
arXiv 2604.11378 found in 75 % of surveyed graph-orchestration systems is
made unrepresentable, matching its three-level escalation and
plan-immutable-within-a-version commitments exactly.

## 10. Prior art, one line each

| source | what it confirms |
|---|---|
| the reference (production) | the ledger/delegation split; auto-promotion; work-triggered nudges; stop interception; content-as-identity |
| the reference (production) | pull over broadcast for peers; single-writer ledger with derived topology; one status formula per fleet; per-child caps as typed spawn data |
| SGH 2604.11378 | versioned immutable plans; bounded escalation; deterministic multi-ready dispatch |
| ADaPT | decompose on failure, as deep as the executor needs — and no deeper |
| LLMCompiler 2312.04511 | dispatch the whole ready set |
| TDAG | choose the agent per task (`SpawnSpec`), not from a fixed roster |
| TDP | scoped context per subtask; replan locally |

## 11. Considered and killed

Recorded because the corpse explains the survivor.

- **A message board** and shared trajectories — the session's founding
  idea; centralized surfaces scale noise, and shared trajectories score
  negatively. Survivor: collision-gated peer channels, pull-based
  inspection.
- **Benchmark profiles**, twice (E6 packs, then a typed policy object) —
  both encode benchmark identity as configuration. Survivor: one
  configuration, the regime diagnosed at runtime (§8).
- **"Run the check" as a foundation** — a passing command is easy to
  supply, fake, or under-scope, and means nothing in ARC. Survivor:
  expectation before action (primitive 3), with `Check` demoted to the
  delegation seam where it belongs.
- **A universal 512-byte message cap**; **human ratification in the
  loop**; **a five-layer file-read predictor** — each replaced by a
  structural mechanism (references, ratchets, fences) recorded in the
  fan-out appendix.
- **Depth = 1** — reevaluated. The hazard was opacity, not depth: with
  decomposition as a parent-visible event, a fully readable DAG, and
  plan-less depth-2 executors, depth 2 is safer than depth 1 with opaque
  children.
- **The stdio CPython REPL** — investigated and rejected (§8.9.1).
  IPython stays long-term; its costs are optimized, not escaped.
- **A closed URL scheme set** — proposed and killed the same day. "Four
  schemes, no more" contradicted the reference type's own point; the set
  is open behind one resolver seam, and what closes is the *terms* —
  permission, provenance, cost — not the schemes (§3.2).

## 12. The ledger's yield

Every item below is a query over the four stores the design already
commits to — plan files, op events, the fetch log, git — none of it new
state. Affirmed from the 2026-08-31 ideation; each names what it rides.

- **`yi why` — code answered to intent.** Every commit main makes under a
  plan carries a machine-parseable trailer naming `plan://<plan>/<todo>`.
  Then `git blame` → commit → todo → goal → `user://<n>`: "why does this
  line exist" resolves to the original user words with zero inference.
  The reverse index is the same data — a todo's ledger row accumulates
  the commits it produced.
- **The outcome ledger, per todo.** Duration, wait, and block time from
  the transition timestamps; tokens and cost from B9 attribution keyed by
  todo; retries and eventual supersession from plan history — and
  **retroactive implication**: when a later bugfix's touched lines blame
  back through `yi why` to a todo in an earlier plan, the host appends
  `implicated-by: plan://<bugfix>/<todo>` to that todo's row. Toil and
  root cause become attributable after the fact, mechanically — nobody
  judges the culprit, the blame graph does. Annotations are
  host-attributed appends; a Done plan gains history, never mutations.
- **Amdahl, measured on every plan.** Critical path through the DAG over
  actual durations, against wall clock: K3's serial fraction — which the
  fan-out doc concedes is "almost never computed" — becomes a free output
  of every run.
- **Discovery ratio.** Todos added mid-flight over todos at init: one
  number that says what kind of work a session was (coding low,
  investigation high, ARC near 1), labeling the offline fit's corpus
  with regimes nobody annotates.
- **Calibration and routing.** Declared effort vs observed duration is
  the self-estimation curve; accept-failure rates per role, model, and
  task cluster are learned routing; blocked time by discriminant tells
  the user where the wall clock actually went.
- **OpenTelemetry for free.** Plan = trace, todo = span, delegation =
  child span, transitions = span events, `after` edges = span links. One
  serializer, and every existing tracing UI renders a session as a
  flamegraph — observability with none of our own UI.
- **Full replay.** Plan file + op events + pinned fetch hashes + the
  eval cassettes replay a session's decision structure deterministically;
  debugging a bad run is stepping through the plan, not spelunking a
  transcript.
- **Decomposition priors from the corpus.** Done plans stay, so
  recurring cuts get mined into skeletons that eager-init proposes and
  the model disposes — and superseded versions are free negative labels,
  the cut that got replaced paired with the cut that survived.
  Decomposition itself learns from the ledger, on data already kept.
- **Plan-aware compaction.** Entries attached to Done todos compact
  aggressively; material for Ready/Running/Blocked todos is load-bearing
  and survives. The ledger tells the compactor what still matters —
  priority stops being recency-only.
- **Duration priors in the HUD.** A todo running far past its
  label-cluster's prior is flagged in the statusline — a threshold on
  data already flowing, surfaced to main, who stays the decider. Not a
  sweep; nothing acts on it but the owner.
- **Semantic plan diffs** — with two caveats that are the design. One
  differ: the function that reconciles user edits into attributed ops
  *is* the renderer of "3 completed, 1 added, 1 superseded" (a second
  parser would be a shadow model); and a supersede renders as one event,
  never as removed-plus-added churn. The issue mirror posts diffs at
  sync cadence, never per op.
- **`yi plan lint` — the doctrine as checks**, bounded by what a lint
  may be: advisory to the owner, never blocking an op (D50: a trigger is
  not a verdict), run at cut boundaries — init and supersede — never per
  op. Mechanical rules are hard findings (a delegation without
  acceptance, DAG width past measured cores, a terminal record with an
  ephemeral URL); judgment rules are soft advisories (vague labels,
  skeleton-first violations); every threshold is fitted on task-shape
  features, never hand-set.
- **The corpus as a kernel dataframe.** Plans are JSON-fronted files; `fetch`
  plus a three-line loader and the model queries its own history
  mid-session — "what did we decide about auth last month" is a Python
  expression over its own past, not a memory feature.
- **Fuzz the state machine from the schema.** A generator produces
  thousands of random op sequences, runs them through the real step
  table, and asserts every invariant after every op — edges acyclic,
  labels unique, no ephemeral URL in a terminal record, no budget at
  depth, supersede only after reaping. The invariant functions are the
  insert-checks reused; `proptest` rides as a dev-dependency (the D71
  pattern). The payoff is shrinking: a failing property auto-minimizes
  to the shortest op sequence that still breaks, and that sequence is
  committed as a fixture, permanently — composite paths no
  transition-by-transition test would try (`decompose`, then `supersede`
  the parent while the sub-plan holds a Running child) surface in
  seconds. The ledger is the one component where a missed illegal
  transition corrupts state itself; it earns this, and the TUI does not.

## 13. What lands, in order

1. `Plan`/`Todo`/`Delegation` shapes in `yi-types` — schema-lock diffs.
2. The `fetch` resolver seam with the internal schemes; walls as
   URL-prefix denies; the fetch log (M3's instrument, and the pin log);
   hashline fragments on `local://` and `checkpoint://`; the
   terminal-seam pin rewrite and unbacked-claim check.
3. The `plan` tool with the §4 op vocabulary; transition table as data.
4. Loop coupling (§6): eager init, nudge counter, stop interception,
   windowed injection.
5. Delegation dispatch over the existing B-series; promotion at collect.
6. `bash()` handles on the IPython kernel.
7. The TUI tree fed by the same structs the runtime reads.
8. Commit trailers carrying `plan://` from the first plan-attributed
   commit — §12's index accrues from day one; the `yi why` command
   itself can land whenever.

Costs: no new crate, no new dependency. Before finalize: the golden-path
walkthrough as fixtures — three real sessions (a typo fix that never opens
a plan; a TB4-scale campaign with one decompose and one supersede; an ARC
game at level grain) written as exact op sequences with asserted states.
Every remaining unknown is in the ops protocol, and that file is also the
first test.
