# Closing the Loop — completion, plan, and reminder architecture

```
status:  APPROVED 2026-08-27; steps 1–4 LANDED at ARCHITECTURE 0.33.0 (D52/D53 —
         goal.check, Fact::Plan + PlanService, doctrine fragment, orchestrate +
         session-mining skills, validator hoist). Open remainder tracked as
         docs/TODOS.md section N and the pre-existing F rows.
date:    2026-08-27
sources: Factory.ai "What it Takes for Coding Agents to Complete Large Software Tasks"
         (2026-08-27) · opencode packages/codemode @ 15537a4 · omp TTSR
         (docs/ttsr-injection-lifecycle.md, docs/rulebook-matching-pipeline.md) ·
         codex plan.rs/plan_spec.rs/collaboration-mode-templates/plan.md @ 2026-08 clone ·
         prime-agent core survey · dogwood-policy/dogwood (survey only) ·
         YI_DESIGN.md §7/§8.10/§8.11/§8.17/§15/§16 ·
         ARCHITECTURE.md decision log · docs/TODOS.md
```

The session that produced this reviewed the Factory result, the opencode code-mode
runtime, omp's stream-rule system, and the codex/prime plan-and-goal lineage, and
pressure-tested each against Yi's settled decisions. What survived is one coherent
architecture. What died is recorded at the end, because the negative space is half
the value.

## 1. Thesis

Factory's result (same model, 24 hard ProgramBench tasks, single-agent median 56.7 →
system median 89.3): the single agent did not lack skill, it lacked a **standard of
completion** — an executable account of what must be true and how to prove it, built
before implementation, held outside the implementer's context, allowed to grow and
never to shrink. Every local judgment was reasonable; the whole was never measured.

Yi's design already believes the underlying principle — **enforced beats recalled**
(§16, TRACE: prose corrections violated 57.5% vs 2–37% when compiled to runtime
checks) — but applies it only to permissions. This plan applies it to completion,
decomposition, and standing corrections, using machinery Yi already has: goals,
session-store facts, the subagent family, the kernel, the permission engine, the
advisor delivery path, and the skills catalog.

Eight principles carry the whole design:

1. **Done is a measurement, not a feeling.** A completion claim is verified by the
   host running a check, never accepted on the model's report alone.
2. **Authority is a capability set, not prose.** (codemode: "do not expose a broad
   tool and expect the prompt to restrict it.") Walls and expand-only rules are
   permission-rule differences, never instructions.
3. **Plan is structure; goal is autonomy.** A plan may be inferred by the model; a
   goal is never inferred (G2 stands). Unattended continuation runs only under a goal.
4. **The enforcement ladder.** A standing correction escalates, always user-gated:
   prompt doctrine (always-on words) → triggered reminder (fires at the moment it
   applies) → hold/deny (blocks at the tool gate). One TRACE mechanism, three rungs.
5. **State-space-as-data, judgment-in-the-model.** The plan DAG, the frontier, the
   rules, the reminders are tables the runtime reads; every *decision* (spawn, plan,
   invoke a skill) stays with the model. The runtime never auto-spawns (D50 spirit).
6. **Intermediates stay in program space.** (codemode; Yi's kernel is already the
   code-mode runtime.) Child results, mining sweeps, and instrument runs live as
   kernel values and files; one bounded digest crosses into model context.
7. **Zero-start.** No builtin rules, no default instruments, no unrequested plans on
   small tasks. Every mechanism costs nothing when unused.
8. **Coverage honesty.** (codemode `PARTIAL — N of M shown`; Factory's licensed
   relaxations.) A check, catalog, or report states what it does not cover; a
   relaxation or skip is named with a reason, never silent.

## 2. Evidence base — one load-bearing fact per source

- **Factory**: gap closed 73% for Fable 5 by adding an independent, executable,
  expand-only completion instrument plus an information wall; budget was not the
  separator — "every single-agent campaign ended because the agent decided to end
  it." Directives crossing the wall were feature-level clusters, never raw results.
- **codex `update_plan` @ HEAD**: still `session.send_event(EventMsg::PlanUpdate)` —
  persists nothing, checked nowhere, and codex bans it inside its own Plan Mode
  ("update_plan is a TODO/checklist tool and is not allowed in Plan mode"). D26's
  anti-lesson is current, not historical. Codex Plan Mode's *template* is the
  valuable half: a **decision-complete** spec — "the implementer does not need to
  make any decisions" — with explore-before-ask discipline.
- **prime-agent**: has no task system (goals + cron + rlm children only). Nothing to
  port; Yi's goal module already superseded prime's via codex (D25).
- **opencode codemode**: independent convergence on the code-mode pattern Yi ships
  as the kernel (§5.2/D36) — one program orchestrates tools, intermediates stay
  in-program, catalog is budgeted and honest, authority is the supplied tool tree,
  failures are data with one safe channel, three limit knobs with no library
  defaults.
- **omp TTSR**: user-authored markdown rules with frontmatter triggers (regex/
  literal `condition`, `scope: tool:edit(*.rs)`, per-rule `repeatGap` in completed
  turns), delivered as verbatim `<system-reminder>` blocks prepended to the matched
  tool's result, or as a mid-stream barge (abort + discard partial + inject +
  retry). One canonical Rule shape bucketed three ways: always-apply / rulebook
  (name+description in prompt, body on demand) / triggered.
- **Yi's own ledger**: 0.11.0 pre-committed the shape of any future plan tool —
  "will persist a session-store fact like `Fact::Goal`" and be checked at turn end.

## 3. Architecture

### 3.1 `goal.check` — the executable completion gate (smallest piece, ships first)

`Goal` gains `check: Option<String>` (additive, §19-safe). On `goal.update{status:
complete}` the host runs the check under the session cwd; nonzero exit rejects the
update and the output tail becomes the blocker text in the next G4 continuation
prompt. The run record persists as the audit trail even when the claim is rejected.

- Authority follows G2 exactly: the model may *report* terminal state; the host
  verifies it. `goal.update` still accepts only `complete|blocked`, so the model
  cannot mutate the check it is judged by.
- One budget knob: a timeout (codemode's lesson — no separate work budget).
- Runs through the existing bash executor path — no second process-spawn surface.
  A failing or timed-out check is a typed rejection carried as the blocker text,
  never a panic path.
- No new privilege: in yolo the model already runs arbitrary bash; the check is a
  command the user's own goal carries.
- D26's rule sharpens: completion is judged by the goal check's exit code when one
  exists. This is the repo's own "done means `just check` green, judged by exit
  code" doctrine, given to the runtime.

### 3.2 `Fact::Plan` — a task DAG under the goal

One new session-store fact beside `Fact::Goal` (same precedent: additive fact kind,
unknown-tag tolerance preserves Pi interop). Never a transcript entry — compaction
cannot lose it by construction (D25 logic).

```
Plan { goal_id: Option<GoalId>, version: PlanVersion, tasks: Vec<Task>, extra }
Task {
  id: TaskId, title,
  acceptance: String,           // terse prose criteria — the instrument's inventory line
  schema:     Option<Schema>,   // structured output contract (the B4 --schema subset validator,
                                //   hoisted out of yi-cli first — yi-runtime cannot depend on
                                //   yi-cli, and the workspace holds one validator, not two)
  check:      Option<String>,   // executable, host-run at the task's done claim
  deps:       Vec<TaskId>,      // DAG; Ready is derived (deps Done), never stored
  state:      Pending | Running | Done | Blocked { reason },
  assignee:   Option<...>,      // Parent | Child{name}; v2 candidate — frontier ignores it
  extra,                        // flattened unknown-field map (§19 durable-struct rule)
}
```

Semantics that make it not-`update_plan` (the D26 test: state must change what the
runtime does next):

- **Host-verified transitions.** A task `done` report runs `check` (when present)
  and validates the child's structured result against `schema`. Failure rejects the
  transition with the evidence as `Blocked{reason}`.
- **Frontier continuation.** G3 generalizes: idle session + Active goal + nonempty
  frontier → the continuation prompt carries the frontier (ready tasks with their
  acceptance, blocked tasks with reasons). A turn ending with work remaining cannot
  terminate identically to a finished plan — the exact codex failure D26 named.
- **Anti-shrinkage at plan level.** Adding tasks is free; deleting a task or
  weakening its acceptance mid-run requires user approval (or an advisor Hold).
  Factory's "the standard must not quietly collapse around what has been built,"
  enforced, not recalled.
- **Single writer.** Plan state is mutated only by the parent session through the
  plan surface (`plan.create` / `plan.update{task_id, state, evidence?}` /
  `plan.edit`); children report, the parent transitions. No races by construction.
- **Plan without goal is legal and inert.** It structures the turn and feeds the
  reviewer; idle still stops. Attaching a goal is the explicit "run unattended"
  grant. Model may create a plan unprompted on a large task (doctrine-guided) or on
  "create a plan"; it may never create a goal unprompted.
- **DAG, not waves.** Frontier scheduling pipelines independent branches; a barrier
  exists only where adjudication genuinely needs all siblings (dedup/compare).
  `frontier(&Plan) -> Vec<TaskId>` is a pure function surfaced in prompts; the
  *model* decides what to spawn.
- **HAR mechanics.** `TaskId`/`PlanVersion` are newtypes; `Plan`/`Task` are durable
  yi-types shapes — flattened extra maps, unknown enum tags decoding to `Other` and
  re-emitting verbatim, committed golden fixtures from the first landing (§19).
  Legal state transitions are one table the runtime reads with an invariant check,
  never scattered branches; every turn, gap, and version counter is saturating.

Relationship of checks: task checks gate task transitions; `goal.check` remains the
whole-goal integration gate (Factory analog: instrument cases vs the final graded
run). A goal with a one-task plan and only `goal.check` is the degenerate case and
costs nothing extra.

### 3.3 The kernel is the orchestration plane (context passing)

Current honest state: `rlm.run` returns a handle at admission; child results arrive
only as B6 assistant-role transcript messages. The upgrade (rides the already-queued
F-rows, one addition):

- **Downstream** (parent → child): task spec rendered through the strict G4
  `{{name}}` template engine (`ExtraValue` — a renamed placeholder cannot silently
  drop content), plus `fork: All|LastN` (F4) for mid-thought handoffs, plus
  `Isolation::Worktree` (F5) for parallel mutators. Delegation guidance (when to
  fan out, when the parent does it itself, env, tone, verbosity) comes from the
  orchestrate skill (§3.9), so the only ambiguity a child faces is the code.
- **Upstream** (child → parent): F3's kernel surface grows `await handle.result()`
  returning the child's structured result **into the parent's kernel namespace**,
  validated against `Task.schema` at the seam. The parent filters and aggregates N
  children's results in Python; only the digest reaches its transcript. This goes
  beyond both refs (prime and codex deliver payloads as messages) and is codemode's
  intermediates-stay-in-program rule applied to subagents. Blockers are fields in
  the same value, consumed by the frontier computation.

### 3.4 Reviewer and the wall

- **Reviewer = cold fork.** Final verification is a subagent spawned with
  `fork: None`, prompted with the plan's acceptance inventory + schemas, running
  each task check and `goal.check`. Cold context is the feature: it cannot inherit
  the implementer's blind spots (Factory's whole result). It is not the advisor
  (which cannot execute) and not the "main agent with semi-cleared context" (a
  compacted context still carries the implementer's framing).
- **The wall is a capability-set difference, two independent layers** (codemode:
  visibility ≠ authorization):
  - **write-deny** on the instrument/plan-criteria paths for implementer children —
    this *is* expand-only enforcement, structural and silent, on by default when an
    instrument exists. Mechanism: B1 overlays already "customize or **reduce** the
    child, never replace parent authority" — a deny rule is a reduction, enforced
    at `decide()`.
  - **read-deny** (plus absence from the child's context) only for *sampled*
    instruments where visibility itself is the Goodhart risk (behavioral parity,
    migrations). `cargo test` is fully visible and fine; the wall is per-goal
    opt-in, not a default.
- **Instrument shape**: case-table-as-data plus one runner under
  `<goal artifacts>/instrument/` — cases as JSON/JSONL, comparator policy explicit,
  per-case results as data, coverage self-statement (`PARTIAL — 41 of ~60 verbs
  probed`), relaxations/skips named and licensed with reasons. Raw results stay
  validator-side; the clustered summary is the single bounded surface that crosses
  (this is all that remains of "directive altitude" — a boundary shape, not a type).
- **Findings reference `Plan.version`** so adjudication is never made against a
  stale standard (codemode's registration-currency check, one field).
- **Benchmark guard (E3)**: the reviewer verifies with the task's own build/tests
  and the plan's checks, never by touching harness verifier paths — TB2.1's judge
  zeroes reward-hacks (a top agent lost 8.99% this way).

### 3.5 Rules unification — one shape, three activations (the omp borrow)

Rules and skills collapse into one canonical markdown-plus-frontmatter shape with
three activation modes; the existing C3 skills catalog is already the middle one
(omp's rulebook bucket confirms the design independently):

```
RuleDoc { name, description?, body,
  activation: Always                          → prompt fragment (context files today)
            | Listed                          → catalog: name+description, body on demand (C3)
            | Triggered { match, scope, gap, mode: Remind | Gate } }
match: Literal(s) | Regex(s)      scope: text | tool:<name>(<glob>)
```

- **Triggered + Remind**: on match, the rule body is delivered **verbatim** at the
  next boundary (tool-source: attached to that tool's result, omp-style). The D50
  thread that makes this legal: the runtime is a *matcher delivering the user's own
  standing words at the moment they apply* — not a reviewer generating judgment.
  Everything the deleted V1 signals got wrong is inverted: rules are user-authored
  files (ship **zero** built in; omp ships 28 — Yi's universals live in the
  doctrine fragment), matches are precise strings the author chose (omp's specimen
  condition is the literal `Box::leak`), the voice is the user's verbatim text, and
  every rule carries a repeat gap in completed turns (omp default 10, `once`
  available).
- **Triggered + Gate — the barge, translated.** omp barges mid-stream because it
  matches argument *streams*; Yi's tool gate already sits after arguments complete
  and before execution, so the only barge that changes outcomes (stopping a side
  effect) is structurally free: a Gate match makes `decide()` return a
  **deny-with-guidance carrying the rule body as evidence** (D26's
  denial-carries-evidence). The model retries informed. No abort machinery, no
  retry tokens, no resume gates. Lost relative to omp: `contextMode: discard`
  (the violating partial never entering context) — deferred D8-style until
  observed need.
- A *skill* with a trigger self-advertises (fires a one-line "consider skill X"
  pointer); a *rule* with a trigger delivers its body. Same file format, one
  discovery walk (project shadows global, as today).
- `astCondition` is not borrowed (no tree-sitter; the brace scanner is for
  hashline). Literal-only v1; regex arrives with the C2 `regex-lite` decision,
  which now has three consumers (rtk corpus, grep regex mode, trigger rules).
- Frontmatter is `runtime::skills::frontmatter()` — the minimal `key: value`
  parser already shipped for the C3 catalog, extended in place; a second parser is
  a duplication-budget violation. A malformed rule (bad trigger, unknown mode) is
  skipped with a named warning, omp's behavior and the config-strictness spirit —
  never silently loaded, never fatal to session start.

### 3.6 Cadence reminders and the noise budget

Second trigger kind: `cadence: every N completed turns`, gated by a predicate from
a **closed vocabulary of two** in v1: `always`, `plan_stale` (plan exists ∧
untouched N turns while tool calls flow). The plan-staleness nudge itself ships as
**plan-feature behavior** (config `plan.stale_reminder_turns`, conservative
default), not a rule file — it is the feature's own state surfacing itself, same
class as the G3 continuation prompt, and its content is data, not opinion:
`plan stale: 4 Ready tasks unclaimed, T3 in progress 22 turns`.

The noise budget — every reminder in the system obeys all five, or it does not ship:

1. **One throat.** Triggered, cadence, plan-stale, and advisor deliveries all route
   through the existing V7 EmissionGuard (FIFO dedupe, one note per cycle) and the
   V8 boundary-injection path. No second delivery mechanism, ever.
2. **Per-rule gap** measured in completed turns; `once` mode available.
3. **L4-wrapped** (`yi_internal_context`) → dropped at compaction; reminders can
   never accumulate into context sediment.
4. **Predicate-silent**: a cadence rule whose predicate is false costs zero.
5. **Verbatim or data.** A reminder is the user's rule text or a machine-generated
   state line. The runtime never editorializes.

### 3.7 Where the advisor sits — three tiers by what each can see

- **Triggered rules**: syntactic, instant, user-authored. Cannot see trajectory
  shape.
- **Cadence reminders**: temporal, structural (staleness). Cannot judge whether
  grinding is *correct*.
- **Advisor**: the judgment tier. The V4 digest header gains the plan frontier and
  assignments, so tunnel vision is visible in the digest ("40 calls on T3 while 4
  Ready tasks sit unclaimed and independent") → `Advise{kind: Scope, target: plan}`:
  "frontier has parallel work; orchestrate." The V9 outcome ledger then measures
  whether the fan-out happened — the tuning loop the deleted signals never had.

Adopted advisor placements: (1) **cadence anchored to plan transitions** — review
fires when a task is claimed done or blocked, restoring a sparse, meaningful
trigger where D50 deleted per-call ones; (2) **PlanAudit** — V5 gains one job:
judge a plan diff for shrinkage ("did this edit redefine success around a smaller
task?"); host enforces mechanics (deletion gate), advisor judges semantics; (3)
**SelectCandidate** (M5, already designed) — N children attempt one task, the
instrument measures each, the advisor *selects*, never synthesizes (D10: selection
81% vs synthesis at chance); (4) **promote** (I1) — a blocker recurring across runs
compiles, user-gated, into a reminder rule or a Hold. Rejected: advisor-as-final-
reviewer — the advisor cannot execute checks; §7.5's "measurements are facts,
advice is judgment" line stays clean.

### 3.8 Session mining — the outer loop (zero Rust)

Yi sessions are typed JSONL with strong structured signals (`is_error` streaks,
permission denials carrying evidence, retries, aborts, V9 advisor outcomes) — a
better mining substrate than the chat-log grepping the idea comes from. v1 is a
bundled skill plus a heartbeat:

- The skill directs the model to **write Python in the kernel** that sweeps
  `~/.yi/sessions`, extracts the structured signals, and clusters them — thousands
  of entries never touch model context; only the cluster table comes out.
- The report states coverage (`N sessions scanned, M skipped (reason)`), appends a
  pain-points ledger, and offers `gh issue` creation through the stateless exec
  pattern already specified for the board (K1), behind ask.
- Output is always a human-gated artifact. Auto-application of fixes, auto-rules,
  auto-skills stay policy-cut. The natural compile target of a recurring pain point
  is a triggered rule — proposed by the report, landed by the user.
- **Rule backtest** (dogwood's event-replay idea, minus its policy language): before
  a proposed trigger rule is enabled, the same kernel program replays its match
  against stored sessions — "would have fired N times; here are the sites" — so a
  rule lands with evidence and a noisy rule is caught before it fires live. Zero
  new machinery; coverage stated as always.
- Promotion from skill to runtime surface requires measured value first (D27
  admission discipline, M3-style instrumentation).

### 3.9 Doctrine fragment and orchestrate skill (no plan mode)

Planning is behavior, not a mode. Yi's permission surface remains **auto | yolo**
(auto deferred to the D1/D2 architecture; ask-as-mode dies with it, but
ask-as-decision — the U12 approval view, C5 bridge, M9 seam — survives as auto's
escalation surface). A non-mutating "plan mode" was rejected: Yi's safety story is
recovery (M10 catastrophic denylist + shadow-git checkpoints + `/undo`), not
prevention, and D44 already ruled on gates nobody declines.

- **Doctrine fragment** — the deferred 0.13.1 slot. Terse, stable, cached (§15.3:
  anything time-varying in the prefix is a per-turn full re-read). Carries the
  distilled operating rules: when to plan (multi-file / multi-constraint /
  ambiguous → draft `Fact::Plan` first; small task → just do it), parent takes
  simple tasks itself, the ladder-of-reuse and root-cause norms distilled from the
  resident modes, verification-before-submit.
- **Orchestrate skill** — long-form method, loaded on demand: decomposition
  heuristics, delegate-vs-do thresholds, wave/DAG shaping, per-child env
  (worktree, tools, rules), search strategy, tone and verbosity contracts, with
  worked examples as in-context learning. Donor text: codex `plan.md`'s
  decision-complete and explore-before-ask discipline, ported adapted; the
  mode-rules enforcement half is not ported (wrong trust model).

## 4. What died, and why (binding negative space)

| idea | verdict | why |
|---|---|---|
| `update_plan`-shaped plan tool | dead | event-only at codex HEAD; D26's "print statement with a schema" confirmed current |
| plan as a permission mode | dead | recovery-not-prevention safety story; D44 logic; planning is judgment + directive |
| instrument-always | dead | Factory's own economics (14× credits); gated to goals, which are explicit-only |
| wall by default | dead | Goodhart risk exists only for sampled checks; write-deny default, read-deny opt-in |
| per-task ratchet scripts | dead | the write-deny permission rule *is* expand-only enforcement; no speaking layer (D50) |
| stream-abort barge (v1) | deferred | gate-level deny-with-guidance captures the side-effect-stopping case at zero machinery |
| waves as barriers | dead | DAG frontier pipelines; barrier only for cross-sibling adjudication |
| runtime auto-spawning scheduler | dead | frontier is data; spawning is model judgment |
| `Directive` wire type | dead | advisory-only shape = D26 anti-pattern; altitude is a prompt/boundary property |
| advisor as final reviewer | dead | advisor cannot execute; reviewer is a cold fork subagent |
| confined JS/Python interpreter | dead | codemode solves multi-tenant confinement; Yi's kernel + B1 reduction is the right altitude |
| builtin trigger rules | dead | zero-start law; universals live in the doctrine fragment |
| `astCondition` triggers | dead | no tree-sitter; literal/regex covers the observed rule corpus |
| policy language / external policy engine (dogwood, Cedar, Rhai) | dead | dependency law (§13.3/§13.5), anti-DSL law (state-space-as-data), fleet trust model Yi does not have; its event-replay backtest idea is adopted in §3.8 |
| implicit goal creation | dead | goal = autonomy grant, stays explicit (G2 verbatim) |
| mining as a Rust surface (v1) | deferred | skill + heartbeat ships today; promotion needs D27 evidence |
| second budget knob on checks | dead | timeout only (codemode: no separate work budget) |
| heartbeat re-verification of done goals | deferred | check-at-claim covers the measured failure (stopping early) |

## 5. Benchmark posture

Two regimes, one architecture, zero cost when unused:

- **TB2.1 / DeepSWE / QnA** (median 900 s, all-or-nothing, wall-clock and turns
  published): plan-of-1-to-3, parent does most work itself, reviewer pass before
  submit (§15.3 lever 4 — verification-before-submit is nearly free accuracy on
  all-or-nothing suites). Fan-out is mostly a liability here; "parent takes simple
  tasks" is load-bearing.
- **ProgramBench-class campaigns** (Factory: 96 h, 14× credits): where the DAG,
  the wall, and the instrument pay. Same machinery, different knobs.
- **ARC-AGI-3**: neither — kernel loop + goal persistence + cheap exploration
  matter; the plan system is not bent toward it.
- Standing cautions: E3 (never touch verifier paths), E4 (draft `answer.txt`
  early), cache-prefix stability (doctrine fragment is stable text; reminders are
  L4-dropped so they cannot churn the prefix).

## 6. Build order

Each step lands with its own D-row, ledger edit, and tests; sizes use TODOS
notation (S ≤ 1 day, M ≤ 3, L larger).

1. **`goal.check`** · S · types + `runtime::goal` + G4 template. Highest
   value-per-line; independently shippable.
2. **Doctrine fragment + orchestrate skill** · S · prompt + markdown. Immediate
   behavior lift, zero risk.
3. **Session-mining skill + heartbeat** · S, zero Rust · bundled skill; heartbeat
   carries it.
4. **`Fact::Plan`** · M/L · types + store + plan surface + host-verified
   transitions + frontier in G3 + plan-stale nudge. Useful before subagent rows
   land (solo parent + reviewer still benefit).
5. **F1→F5 as queued** · substrate · F3 additionally grows `handle.result()` with
   schema validation at the seam (the hoisted B4 validator).
6. **Rules/triggers unification** · M · discovery-walk extension + literal matcher
   + Gate arm in `decide()` + delivery through V7/V8. Independent of the plan
   except the `plan_stale` predicate.
7. **Reviewer flow + wall** · M · cold-fork reviewer skill + B1 deny-rule overlay
   + instrument conventions.
8. **Advisor jobs** · M · name the advisor model role, construct `LlmReviewer` in
   `attach_runtime`, plan-transition cadence, PlanAudit, then M5 SelectCandidate.

## 7. Doc obligations and open decisions (user calls)

- **D-rows required** (one per landing, why-text drafted in this doc): goal.check
  (revises G2 — host verifies terminal reports); Fact::Plan (revises §8.17 scope;
  plan-may-be-inferred / goal-never-inferred); rules unification (the D50 thread:
  matcher-delivering-user-words vs reviewer-generating-judgment); wall overlay
  (B1 reduction as enforcement); reviewer/instrument conventions.
- **§1.1 one-in-one-out**: the plan system and the rules unification are plausibly
  one top-level feature each. Demotion candidates, in recommended order: M4
  embedding API, L2/L3 remote-rendered UI + pi-compat shim, M1 docs conversion.
  Decision pending.
- **C2 regex decision** (`regex-lite` §13.3 row): now has three consumers (rtk
  corpus, grep regex mode, trigger rules). Recommended: take it when step 6 lands;
  literal-only until then.
- **Auto mode** (D1/D2): deferred as directed; when taken, its D-row also retires
  ask-as-mode (`--confirm` remap, C8/H2 naming, M11 fragment count) while keeping
  ask-as-decision.
- **One-line design-doc addendum** when convenient: §5.2's CLI-shaped MCP is the
  code-mode pattern, independently convergent with opencode — strengthens D36's
  "the real restraint is architectural."
- **Crate budgets**: plan + rules land inside `yi-runtime`'s 6,000-line ceiling and
  `yi-types`' 3,000 or trigger a deliberate D43-style ceiling decision — measured at
  landing, never assumed. The B4 validator hoist moves lines, it does not add them.
- **ARCHITECTURE.md**: no version bump for this file's existence; each landed step
  bumps with its own changelog row per the workflow rules.
