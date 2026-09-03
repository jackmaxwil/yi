# Yi self-optimization — plan v5 (prompt-flywheel lineage)

```
status:  LANDED 2026-08-29 at ARCHITECTURE 0.70.0 — D75 task checks · D76
         behavior gate · D77 decomposition protocol. v1 naive optimizer →
         killed by self-review; v2 lean flywheel → killed by the reference
         autopsy; v3 deterministic rules → demoted by the benchmark
         pressure test; v4 built the instrument and put behavior under the
         gate law; v5 adds the piece v4 hand-waved: a deterministic
         decomposition protocol (§6), studied against two new papers, seven
         codebases, and the planning/agents literature.
landed:  §15's placeholder ids map to what the tree took, since several were
         already claimed when this landed: P8→P12 · J3'→J3 ·
         J11→J10 (TODOS `J11` is now the campaigns/GEPA row) · J1'/J2'→J1
         and J2 · F10→F8 · P10→P13; N11, N12 and N13 kept their ids.
         D-rows: draft D74→D75, draft D76→D76, draft D77→D77, and draft
         D75 (GEPA) was never claimed — campaigns stay open as TODOS `J11`
         until S2 ledger rows exist. One design deviation: `get_context`
         landed as a builtin rather than on P3's `Effect::RegisterTool`,
         which P3 has not built.
date:    2026-08-29
sources: research (ref/research/): 2601.04055v1 MPO · 2603.21520v1 MemAPO ·
         2606.04465v1 SePO · 2507.19457v2 GEPA (ICLR 2026) · Factory.ai
         large-tasks PDF · 2608.26263v1 SKILL.state (Google/Purdue) ·
         20885-AAAI26.KawaseY-GT (sequential selling with sunk cost bias)
         · the reference harness: arXiv 2608.23552 + primeintellect.ai/blog +
         docs.arcprize.org/methodology (RHAE) + arcprize.org/leaderboard
         · the reference autopsy (§0; ref/agents/the reference, excise respected)
         · pattern donors, NOT Appendix A (ports need an A-entry first):
         dsrs + gepars (ref/optimizers/) · ref/orchestration/
         {pi-dynamic-workflows, tinyflows [GPL-3.0 — patterns only, never
         code], sayiir, swarms-rs, tsumugi, rk8s} · the reference
         packages/codemode (excise respected)
         · literature (§6 evidence): ADaPT 2311.05772 · RSTD 2605.15425 ·
         Planetarium 2407.03321 · LLM-Modulo 2402.01817 · ChatHTN
         2505.11814 · LLMCompiler 2312.04511 · Agentless 2407.01489 ·
         mini-swe-agent · TDFlow 2510.23761 · SWT-bench 2406.12952 ·
         self-verification limits 2402.08115/2310.01798 · AbstentionBench ·
         Overthinking 2502.08235 · ToolMaze 2606.05806 · Tree Search
         2407.01476 · MAST 2503.13657 · equal-budget MAS ablation
         2604.02460 · METR horizons 2503.14499 · Anthropic multi-agent
         blog · Cognition "Don't Build Multi-Agents"
         · benchmarks (ref/benchmarks/, A.12 verified): harbor, pier,
         terminal-bench-2-1, SWE-Atlas, ARC-AGI-3-Agents
         · Yi: ext/{mod,assemble,install,orchestrate,telemetry,project,
         grid}.rs, advisor/{mod,review,digest}.rs, {rules,plan}.rs,
         goal/mod.rs, subagent.rs, mailbox.rs, session/jsonl.rs,
         types/{entry,advisor,config,plan}.rs, permission/catastrophic.rs,
         tests/request_budget.rs, skills/yi/session-mining/SKILL.md,
         python/yi_runtime (rlm), YI_DESIGN §2/§7/§13/§15/§19/A.12,
         TODOS §E/§F/§I/§J/§N/§P, docs/plans/2026-08-29-praxist-lessons.md
```

## Laws (carried through every revision; violations are bugs)

1. **Invariant zero.** The user's prompt is never rewritten, paraphrased, or
   reordered (§7.6 mempalace, P17 retention floor). Optimization touches
   Yi's own assets only.
2. **Deterministic control.** Triggers, admission, eviction, selection,
   scheduling, and control flow are exact strings, counters, and
   thresholds. The LLM writes gated prose and generates candidates and
   proposals; it never decides when anything fires, persists, ships, or
   dies. Scope: this machinery. The single recorded exception elsewhere in
   the tree is D81's permission reviewer — role-gated off by default,
   reached only where the deterministic ladder has already stopped the
   call, structurally barred from credential reads, holds, configured
   rules and catastrophic targets, and denying on anything but the literal
   `allow`. Any other LLM in a control path is still a bug; a second
   exception needs its own D-row, not this sentence.
3. **No unbounded loops.** Nothing is scheduled. Mining and campaigns are
   user-run. CI runs only the zero-API deterministic tier. Real-model
   rollouts are budgeted, deliberate, and ledgered.
4. **Cache discipline.** Nothing per-prompt touches trusted blocks (J4/J5,
   0.62.0 breakpoints). Reminders ride the transcript; project text rides
   the yard; no background job rebuilds a prompt mid-session.
5. **Nothing ships unmeasured.** A behavior change is a commit; a commit
   answers to the instrument (§3) and the ratchet (§7).

## 0. Lineage (compressed; the negative space is most of the value)

- **v1 → v2 (self-review):** per-prompt trusted retrieval died to cache
  economics; store/journal DTOs died to `.yi/rules` + D59 promote; the
  recon spec-compiler died to `Task.acceptance` + expand-only `plan.edit` +
  `goal::run_check`; GEPA acceptance was unexecutable from stored traces —
  **acceptance requires execution**.
- **v2 → v3 (the reference autopsy):** the reference's TTSR/auto-memories/auto-skills devolve
  for mechanical reasons — full-context clone per capture with a fresh
  cache key (sdk.ts:1145-1166), write-only stores with no outcome ledger,
  uncapped per-turn renders (one skill ≈ 16k tokens/turn), buffer-regex
  firing on the mention not the act, model-gated persistence with no human
  gate. What survives in the reference is exactly its deterministic subset; v3
  rebuilt on that subset.
- **v3 → v4 (benchmark pressure test):** a rule needs repetition, rots in a
  fast repo, and was measured by an unpowered diary. v4 inverted: the unit
  of learning is an **eval case**; the instrument (adapters + ledger +
  behavior ratchet) comes first; per-task campaigns chase the extreme
  ends; ARC re-enters via the inherited RLM mechanism.
- **v4 → v5 (decomposition study):** v4 said "decompose first — ambiguity
  is metric debt" and stopped there. Studied against SKILL.state, Kawase,
  seven orchestration codebases, and the planning literature, the brain
  dump's instincts validated 7-of-9 — with one inversion: **upfront
  exhaustive DAG compilation loses to lazy, check-triggered splitting**
  (ADaPT +28pts for as-needed; RSTD: static upfront splits cost up to
  +80.5% retry tokens over monolithic; Planetarium: ~75% of unvalidated
  LLM formalizations subtly wrong). §6 is the resulting protocol.

## 1. The inversion

Yi's code cannot ship unmeasured: `just check`, shrink-only ratchets,
golden fixtures, red-first regression tests. Yi's *behavior* — prompt
files, tool descriptions, routing thresholds, subagent briefs,
decomposition policy — is the last unmeasured surface in the repository.
v4/v5 close it. **The unit of learning is an eval case, not a rule**: a
real failure becomes a deterministic replayable scenario with a pass
condition — seen red, fixed, kept green forever. One occurrence suffices;
cases don't rot; pass/fail is external; cases are committed to the repo.

## 2. What the tree provides (verified)

| need | mechanism | where |
|---|---|---|
| adapters, span-exact | harbor/pier subclass contracts, out-of-tree registration, reward + error-classification | YI_DESIGN A.12 |
| ancillary metrics demanded by the harness | pier `AgentContext`: `n_agent_steps`, `peak_context_tokens`, `summarization_count`; TB2.1 pass@k schema | A.12 |
| official ARC scaffold, local | ref/benchmarks/ARC-AGI-3-Agents | A.12 |
| offline replay substrate | faux provider; J3 cassettes row | TODOS J3 |
| the ARC-class engine | kernel + `rlm` package: persistent IPython, context-as-variable, `rlm.run` → host_request → spawn handle | crates/kernel · subagent.rs:760 |
| child transport, today | `rlm.run` kwargs: `fork: none\|all\|N-turns`, `isolation: none\|worktree`; `rlm.wait/result/interrupt/list/delete`; `rlm.merge_worktree`; child cap with mandatory reap | subagent.rs:138,417,707-816 |
| child messaging | mailbox: `ParentLink::send/roster/route/interrupt/result` | mailbox.rs:50-292 |
| DAG substrate | `Task { title, acceptance, deps, state }`, frontier, `PlanVersion`, expand-only edits (weakening user-gated) | plan.rs:17 · types/plan.rs |
| executable check + evidence | `goal::run_check`: exit-0-only, 2,000-char tail | goal/mod.rs:44 |
| advisor sees the plan | `summary_line(plan)` in the digest header | plan.rs:41 |
| routing gate | `Route` prefilter (−3/+4); D72 effort ladder | ext/orchestrate.rs:70 |
| orientation chassis | grid ext; P3 native grid tools + SessionStart `Effect::RegisterTool` | ext/grid.rs · TODOS P3 |
| corpus, per project | `~/.yi/sessions/<cwd-slug>/*.jsonl`; `ext_record` telemetry | session/jsonl.rs:25 |
| mining skeleton, promote gate, rules, redaction | session-mining skill · D59 promote · D54 rules (`gap: once`) · `command_reads_credentials` | skills/ · advisor/mod.rs:420 · rules.rs · catastrophic.rs:154 |

## 3. The instrument

**Micro-evals** (deterministic, zero API, in the gate): faux-provider
cassette replays of pinned scenarios — distilled session failures plus the
harness behaviors benchmarks stress (compaction survival, edit-loop
recovery, claim-vs-evidence at done, repeated-call rejection, orientation
sufficiency, and now: escalation-ladder firing, discovery-ledger drain).

**Task evals** (real rollouts, budgeted, ledgered): `yi_harbor` +
`yi_pier` over TB 2.1 / SWE-Atlas / ARC-AGI-3 subsets. Run-id + config
fingerprint + §4 metrics per row in docs/eval-ledger.md. Timeouts never
retried; a failed run is a result.

**Single-task campaigns** (Factory/GEPA inference-time mode; NPUEval
4.25→30.52% precedent): pin one task, iterate levers. Campaign learnings
ship only as general assets that hold suite-wide — the regression gate
refuses leaderboard hacks.

## 4. Metrics, levers, targets

Metric axes (one row per session and per eval run; sources: session JSONL,
`ext_record`, pier/harbor channels):

- **A. Outcome:** pass; pass@k; first-attempt pass; best-of-n
  selection-by-check rate; completion-claim precision; regressions;
  unforced errors.
- **B. Spend:** tokens in/out; uncached share; cache read_ratio; USD;
  wall; TTFT.
- **C. Orientation:** pre-first-productive-action calls/tokens/turns;
  packet sufficiency; wasted-read ratio; re-read-after-compaction.
- **D. Action economy (RHAE axis):** total and per-class tool calls;
  repeated identical calls; failed streaks; revert churn; **action
  efficiency = (best-known ÷ this-run actions)², cap 1.15**.
- **E. Delegation:** brief bytes vs child-consumed; result bytes vs
  parent-used; child tokens/pass per delegated unit; fan-out/depth
  utilization; **measured delegation overhead multiple** (lit. prior:
  4–15×).
- **F. Context:** peak tokens; compactions; retention-floor hits; **L2
  spill utilization** (state parked in kernel vars).
- **G. Long-horizon:** turns; plan coverage (tasks with checks); checks
  before done; stall/loop detections; steer/interrupts; **split rate,
  node-local retry cost, premise-churn (assumption tasks re-opened),
  discovery counts (HIGH vs deferred), drain-gate hits**.

Levers (all values land as prompt-asset commits or fitted constants, P4
pattern): 1 prompt assets · 2 route→effort map (D72) · 3 retry/selection
by check · 4 `get_context` composition (§5) · 5 delegation contract (§6
transport + `rlm` depth/fan policy) · 6 context policy incl. kernel-spill
· 7 affordance thresholds (orchestrate consts) · 8 kernel utilization ·
9 rules/lessons tail · **10 decomposition policy (§6): attempt cap A,
depth/width caps, escalation thresholds, fitted delegation-overhead
constant**.

Targets: **T1 frontier** (hardest, pass at any cost, then ratchet cost) ·
**T2 efficiency** (easiest, minimize B+C+D at pass=100%) · **T3
orientation** · **T4 delegation** · **T5 ARC** (§11) · universal exit
gate: suite regressions 0 or the diff does not ship.

## 5. `get_context()` — one call, oriented

One tool call returns a layered, budgeted orientation packet: (1) grid
survey slice; (2) symbol neighborhood (`grid_uses/scope/resolve`); (3)
file skeletons; (4) git change heat; (5) gate commands; (6) prior-issue
fingerprints from mining. Chassis: P3 `Effect::RegisterTool` at
SessionStart. Each layer clamped with named truncation; layer inclusion
fitted from orientation mining. Codemode contributes the projection
discipline: tool signatures rendered as **typed Python stubs** in the
kernel (the model calls what it reads), a token-budgeted catalog with an
**honest completeness header** (`COMPLETE` vs `PARTIAL — N of M`), and a
deterministic search fallback. Embeddings stay banned until C metrics
prove deterministic orientation fails (§1.1's own condition, finally
measurable).

## 6. The decomposition protocol

### 6.1 Evidence verdict on the design inputs

| input | verdict | decisive evidence |
|---|---|---|
| falsifiability as the split criterion | **cornerstone** | TDFlow 94.3% w/ human tests vs 68.0% self-written — the check is the bottleneck; all measured gains sit in sound external verification; LLM self-critique is net-negative (2402.08115, 2310.01798) |
| task-as-code, typed per-node state | **validated** | SKILL.state: (spec, Σ, obs) prompt, reasoning discarded after validated JSON patch; 0.94 vs 0.18 at equal token budget; noise filtered at patch time; 0 recovery steps vs 5–14 for history runtimes |
| upfront exhaustive DAG compile | **inverted** | ADaPT: as-needed beats always-decompose (+28pts max); RSTD: static splits +80.5% retry tokens over monolithic; Planetarium ~75% unvalidated formalizations subtly wrong |
| LLM critic judging splits | **replaced** | the question ("can a non-LLM program prove it?") becomes a *structural* gate; tinyflows: deterministic pre-write gates refuse only the guaranteed-wrong; the advisor may advise on check quality, never gate |
| scoped child transport | **validated; half-built** | MAST: ~42% of multi-agent failures are specification failures; Yi's `rlm.run` has fork/isolation/wait/result — the missing axis is key-scoped state |
| durable discovery ledger | **validated precisely** | SKILL.state limitation (2): an observation not committed to state is gone — so discoveries are a **reserved schema field**, not a scratchpad; tinyflows LedgerRow; pi-dyn's store dies at run end (the predicted flaw) |
| fidelity formula F, yield formula Y | **not computable; replaced §6.5** | F's numerator is gameable, denominator doesn't pre-exist; Y's terms are LLM estimates. Empirical sizing + fitted overhead constants instead (ADaPT, METR, Anthropic 4–15×, equal-budget MAS ablation) |
| anchoring as the early killer | **validated + theory** | Kawase: rising abandon-threshold ⇒ quadratic loss, possible never-abandon; pricing one's own bias ⇒ linear; AbstentionBench: premise-checking must be imposed (reasoning training worsens it); ToolMaze: recovery scales 3.66× slower than skill |
| more agents as accuracy strategy | **rejected** | equal-compute single agent matches/beats every MAS (2604.02460); Anthropic's own coding caveat; depth is for isolation + parallel breadth only |

Donor patterns (read-only; tinyflows is GPL-3.0 — patterns, never code):
tinyflows recipe→graph deterministic lowering, collect-all pre-write
gates, dry-run diagnostics for what green hides, stall-rule stopping,
cross-run ledger with approach-signature exclusion · pi-dynamic-workflows
task-keyed journal + longest-unchanged-prefix resume, nondeterminism
neutered as a resume precondition, store deltas with per-key rollback,
byte-ratcheted prompt surface, schema-noncompliance-is-fatal · sayiir
id-newtype family + topology fingerprint refusing resume-on-changed
definition (`PlanVersion` is Yi's seam) · rk8s dagra note: bounded
channels as fan-out backpressure · swarms: static path/deadlock analyses.

### 6.2 The six laws

**L1 — Check-first admission.** A task enters `Running` only with an
executable check (`Task.check`, D74). Checkless nodes are `ask`s or
explicitly-unmeasured leaves the advisor names. Intake extracts the
boundary set (journeys, red/green assertions, performance ceilings,
output shape) into plan fields; holes become questions — never invented
constraints (hallucinated criteria poison everything downstream).

**L2 — Lazy splitting.** Attempt the whole node first; split only after
its check stays red through A attempts (A ≈ 2, fitted later). The model
proposes splits through a constrained recipe; deterministic code validates
and lowers it onto the plan DAG (§6.3). The model never writes topology.
Expand-only law already guarantees splits refine and never weaken.

**L3 — Typed state transport.** Child invocation is the SKILL.state
triple: immutable task spec + scoped state (kernel variables named by
`context_keys` — bulk data stays in L2 as handles, never inlined) +
latest observation. The child returns a schema-validated result whose
schema reserves a mandatory `discoveries` field. Child reasoning is
discarded. A malformed result is fatal and never silently null (the
pi-dyn hazard, inverted).

**L4 — Structural escalation.** Counters, not prompts: A consecutive reds
on a node force a state change — split, backtrack the DAG edge, or ask —
never "try harder." Same-fingerprint failure streaks trigger premise
re-check before any descendant spends more. Where a check can adjudicate,
lowest-overthinking-of-k selection applies. Recovery is protocol: it
scales 3.66× slower than task skill, so the model will never bring it.

**L5 — Discovery ledger with a drain gate.** Criticality is derived, not
declared: a discovery is HIGH iff it names a violated check of an
ancestor task — and the runtime can *run that check* to confirm. HIGH ⇒
pause/fork now. Everything else ⇒ a deferred row with provenance. **The
goal cannot complete while undrained HIGH rows exist** — the "durable
reminder" is a completion gate, not a timer, so Law 3 holds and nothing
is lost.

**L6 — Sunk-cost sophistication (Kawase, mechanical).** Continue/abandon
decisions read current state only — invested tokens are structurally
invisible (SKILL.state makes this free: the prompt carries Σ, not
history). Abandon thresholds are stationary and fixed a priori. Each
consecutive red escalates the effective price of continuing (remind →
forced split/backtrack → abstain-and-ask): the sophisticated-agent
construction that converts quadratic worst-case loss to linear.

### 6.3 Schemas (yi-types additions, additive, HAR-shaped)

```rust
/// Split proposal — the ONLY surface on which the model shapes topology.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtaskSpec {
    pub title: String,
    /// L1: absent ⇒ this child must be an ask or a marked-unmeasured leaf.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// Kernel/state keys the child may read (L3 transport scope).
    pub reads: Vec<String>,
    /// Declared write set; disjointness gates parallelism.
    pub writes: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Collect-all validation (tinyflows gate shape): refuse only the
/// guaranteed-wrong, name every failure in one round trip.
#[derive(Debug, Clone, PartialEq)]
pub enum SplitRefusal {
    UnknownReadKey { index: usize, key: String },
    OverlappingWrites { a: usize, b: usize, key: String },
    DepthExceeded { depth: u8, max: u8 },
    WidthExceeded { width: usize, max: usize },
}

/// Child result. `discoveries` is mandatory (may be empty): committing
/// out-of-scope findings is part of the patch contract, not a heroic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildResult {
    pub value: serde_json::Value,
    pub discoveries: Vec<Discovery>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub text: String,
    /// Names an ancestor task whose check this violates; the runtime
    /// re-runs that check to confirm — criticality is derived, never
    /// model-chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub violates_check_of: Option<String>,
    pub fingerprint: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

Transport lands as two additive `rlm.run` kwargs (`context_keys`,
`check`) on the existing handler (subagent.rs already rejects unknown
kwargs by name, so the surface is closed); mailbox carries `Discovery`
rows; deferred rows drain into the session as custom entries and surface
in the mining issue board (§9).

Escalation is a table, not a flow:

```rust
pub enum Escalation { Retry, ForceChange, Ask }

/// Incident: prompted "reconsider" does not fire; counters do.
/// Stationary by construction — no field of this decision reads spend.
const LADDER: [(u8, Escalation); 3] =
    [(1, Escalation::Retry), (2, Escalation::ForceChange), (3, Escalation::Ask)];
```

### 6.4 Route gating and honesty

`Route::OneShot` never sees the protocol — mini-swe-agent's >74% Verified
with 100 lines says strong models need little structure on known
distributions. The protocol exists for the Complex/long-horizon regime
where Factory, RSTD, and the reference field report show unstructured agents
collapse. Every law is a lever on the instrument; belief decides nothing.

### 6.5 Replaced formulas

- **Task sizing** (was F): a node is right-sized iff it has exactly one
  falsifiable check, the executor passes it within A attempts, and it
  sits under the model's measured horizon (METR: ~1h human-equivalent at
  50% in 2025, doubling ~7 months — re-fit per model from the ledger).
  Too small = check is trivially green on attempt 0 with delegation
  overhead > execution cost (E metrics detect it).
- **Fork decision** (was Y): fork only when (a) the node is check-defined,
  (b) parallel siblings have disjoint `writes` (or worktree isolation),
  and (c) expected context isolation exceeds the **fitted** delegation
  overhead constant (E-axis telemetry; literature prior 4–15×). Depth
  stays 1 (current `rlm.run` reality) until a T1 campaign shows a task
  class that needs 2.

## 7. The behavior ratchet and the eval ledger

`guardrails/behavior_baseline.json`: micro-eval case list + pass states;
shrink-only; runs in `just check` on the faux tier; proven by neutering
(D76). `docs/eval-ledger.md`: one row per task-eval run — run-id,
suite@rev, config fingerprint, §4 metrics. Claims cite ledger rows or
they are vibes.

## 8. Candidates are commits

Optimizer and miner output is a diff; git is store, review is gate,
ratchet is regression net. Closed four-trailer vocabulary
(check_comments-style closure):

```
Tune orchestrate route weights from run 0142

Middle-band prompts misrouted to Complex; T2 campaign, easy suite.

Opt-Run: 0142 suite=tb21-easy@a3f model=opus-5 effort=high
Opt-Delta: pass 48/48=; tokens -21%; wall -18%; regressions 0
Opt-Lever: crates/runtime/src/ext/orchestrate.rs route weights
Opt-Cases: +mined-0093
```

Baseline/ledger updates ride the follow-up `--update` commit; no
assistant co-author trailers; lint row only after ~10 real commits.
pi-dyn's capability-contract → generated-docs release gate is the pattern
if the trailer set ever grows.

## 9. Session mining, architected

Patterns: Sentry fingerprint→issue lifecycle (NEW → CASED → FIXED(commit)
→ REGRESSED → RETIRED, deterministic transitions) · ELT (raw JSONL is
immutable truth; versioned extractor; derived stores are disposable
caches) · aviation blameless reporting (coverage line, labeled
uncertainty) · the reference's Continual Harness with auto-persist inverted
to a human gate · TRACE (corrections compile to checks, not prose) ·
GEPA/MemAPO/SePO reflection records, 1:1 balance, verify-before-update.

Mined per session: μ row (§4 axes) · failure events `{tool, fingerprint,
args_hash, resolved_in_session, resolution_action}` · friction events
(asks, retries, reverts, steer/interrupts, correction turns via the §7.6
lexicon) · orientation trace (feeds §5 layers) · delegation exchanges
(feeds the §6.5 overhead constant and sizing fit) · **premise events
(assumption tasks opened/re-opened; escalation-ladder firings)** · wins.

Derived stores (`.yi/mining/`, gitignored, re-derivable): mu.jsonl ·
issues.jsonl · orientation.jsonl · delegation.jsonl. Outcomes ranked:
**eval case > tool/affordance TODOS row > doctrine/prompt edit > fitted
constant > trigger rule**. Nothing lands without the user.

## 10. Redaction

At extraction and on every artifact, case, ledger row, and reflection
prompt: credential-store path lines dropped (`command_reads_credentials`
— one vocabulary, second use); `authorization|bearer|api[-_]?key|token|
secret|password` lines masked; ≥16-char mixed `NAME=value` and ≥32-char
high-entropy tokens masked (SHAs/ulids caught — acceptable, noted);
`$HOME` → `~`. Planted-fake fixture proves the pass.

## 11. ARC-AGI and the kernel

the reference harness reports 95.5% ARC-AGI-3 RHAE Best@1 (self-reported, replay
published; official top base model: Opus 5 native 30.2%; NVIDIA AVO and
Schema claim similar-or-higher). Mechanism: no ARC-specific workflow —
persistent REPL holding game state as variables, exploration and
verification as code, `rlm.run` recursion, a PRO-LONG-style autonomous
prompt. Yi ported exactly this mechanism at phase 4, so it inherits the
capability class; the §6 protocol composes naturally (ARC train pairs ARE
per-hypothesis executable checks — L1 applies unchanged). Levers: §4
5/6/8 + the autonomy prompt. RHAE's formula is adopted repo-wide as the
D-axis metric. Honesty rails: nothing claimed until run on the official
scaffold with published replays; missing kernel-manager pieces become
K-rows with numbers.

## 12. Online deterministic layer

`Task.check` (additive; schemas.lock + fixture): `plan.update(done)` runs
it via [`goal::run_check`]; red refuses with the evidence tail. Advisor
digest counts check-less done-claims (one sentence added; no Reviewer
enum). D50-clean: completion enforcement the model opted into. On top:
bounded retry-on-red and best-of-n-selected-by-check (a check is a free
judge) — each measured before default-enabling. §6's ladder, split
validation, transport kwargs, and drain gate extend this layer.

## 13. Stages, gates, kill criteria (instrument first)

- **S0 — mining foundation** (skill + kernel python; no Rust): extraction
  schema, signal census, issue board, redaction plants, first distilled
  scenarios. Kill: no distillable failures.
- **S1 — micro tier + ratchet** (J3 + D76): cassettes over faux;
  behavior_baseline in `just check`; neutered-gate proof. Kill:
  nondeterministic cassettes.
- **S2 — adapters + ledger** (J1/J2 shaped): yi_harbor/yi_pier, §4 metric
  emission, eval-ledger, first TB2.1/SWE-Atlas/ARC rows. No kill — this
  stage only reveals.
- **S3 — online layer** (N11 + §6 foundations): Task.check + red-refusal;
  escalation ladder; retry/best-of-by-check; route→effort map. Each lands
  with an S2 delta and zero regressions.
- **S4 — protocol + campaigns + get_context**: split recipe + validation,
  `context_keys`/`check` transport kwargs, ChildResult + discoveries,
  drain gate; T1/T2 campaigns; §5 packet on P3. Kill per §6.4: any law
  that fails to move its lever's metrics on the instrument is dropped as
  a law and kept as an option.
- **S5 — GEPA proper** (D75): round-robin module optimization over the
  instrument; candidates-as-commits; ≤ 92 reflection calls/run. Kill:
  the frontier-headroom test.

## 14. Guardrail and ratchet impact

| dimension | S0–S2 | S3 | S4 | note |
|---|---|---|---|---|
| deps / crates / `YI_*` env vars / CLI verbs | 0 | 0 | 0 | adapters are python under evals/ and read two harness variables, `EVAL_BINARY`/`EVAL_BINARY_URL` — outside the binary's env surface, and named without `YI_` so the surface gate cannot mistake them for one |
| src LOC | 0 (+cassette/gate glue at S1) | ≈ +180 (check gate + ladder) | ≈ +400 (recipe validator ~150 · transport kwargs + ChildResult ~120 · drain gate ~60 · counters ~80) | ceilings per module; `--update` own-commit law |
| yi-types | 0 | +1 field | +3 shapes (§6.3) | schemas.lock + fixtures same commit |
| prompts/.md | 0 | +1 sentence | recipe surface rides the orchestrate/subagent fragments, byte-ratcheted (pi-dyn precedent: 800 B prompt budget test) | duplication 0 |
| request-prefix bytes / API in CI | 0 | 0 | 0 | laws 3/4 |

## 15. Rows and D-rows (placeholders; renumber against the live file)

- `P8` mining foundation · M · session-mining + kernel python (§9). done:
  real-corpus report, coverage line, plants absent, ≥1 distilled scenario.
- `J3'` cassettes serve the micro tier. done: one mined case replays twice
  byte-identical.
- `J11` behavior ratchet · S (needs D76). done: red case blocks
  `just check`; neutered proof.
- `J1'/J2'` adapters + ledger. done: a ledger row per suite with config
  fingerprint.
- `N11` task checks · M (needs D74). done: red-check refusal + neutered
  proof + fixture + older-reader passthrough.
- `N12` escalation ladder + premise re-check · S · plan.rs + subagent.rs
  (needs D77). done: ladder fires on a scripted red streak in a cassette
  case; no decision input reads spend.
- `F10` scoped transport · M · subagent.rs + types (needs D77) —
  `context_keys`/`check` kwargs, ChildResult with mandatory discoveries,
  malformed-result-is-fatal. done: child sees only named keys (test:
  enumeration attempt fails); discovery with a confirmed violated check
  pauses the parent.
- `N13` drain gate · S · goal/mod.rs (needs D77). done: goal completion
  refused while an undrained HIGH row exists; drained rows land as custom
  entries the mining board reads.
- `P10` get_context · M · rides P3 + orientation mining. done: packet
  registered at SessionStart; orientation delta measured on T2.
- Campaign/GEPA at S4/S5: landed as TODOS `J11`, open and gated on `J1`/`J2`
  baselines. Its D-row (this draft's "instrument-driven optimization") stays
  unclaimed until those rows exist; the flywheel landing consumed the numbers
  this draft reserved, so claim one against the live header, never from here.

D-row drafts (texts only; claim numbers at land after re-reading the
header):

- **D74 — executable acceptance on plan tasks** (§12; reversible-via:
  drop the field read + header count; field survives as unknown data).
- **D75 — instrument-driven optimization** (campaigns + GEPA, parked
  until S2 baselines; candidates are commits; exit gate = zero suite
  regressions).
- **D76 — behavior baseline gate** (shrink-only micro-eval ratchet in
  `just check`, faux tier only; reversible-via: remove gate + baseline;
  cases remain ordinary tests).
- **D77 — the decomposition protocol** (§6): check-first admission, lazy
  recipe-validated splitting, key-scoped typed transport with mandatory
  discoveries, derived criticality + drain gate, counter-driven
  escalation with stationary thresholds. Why: ADaPT/RSTD/TDFlow/MAST/
  SKILL.state/Kawase per §6.1; the model proposes, code disposes.
  Reversible-via: every piece is additive on plan/goal/subagent; dropping
  the validator returns `rlm.run` to its current kwargs.

## 16. Not building (accumulated; each with its killer)

- Live prompt rewriting; per-prompt GEPA (invariant zero; negative-EV).
- Scheduled/heartbeat anything; LLM-judged firing/persistence/eviction/
  control flow (Law 2's scope, and D81's permission reviewer is its one
  recorded exception); TTSR-style stream interrupts; uncapped renders;
  write-only stores (the reference autopsy).
- **Upfront exhaustive DAG compilation** (ADaPT/RSTD/Planetarium — §6.1);
  **the F and Y formulas as written** (not computable; replaced §6.5);
  **an LLM critic gating splits** (structural gate instead); **LLM juries
  for verification** (pi-dyn `verify`/`judgePanel`: control flow judged
  without ground truth); **a second interpreter/sandbox** (codemode's
  3.5k lines — the kernel already exists); **scratchpad discovery
  channels** (run-scoped stores die at run end; reserved schema field +
  drain gate instead); **depth > 1** until a campaign shows the task
  class; **silent-null failure propagation** (malformed = fatal).
- reference-style auto-persisted continual state; per-prompt trusted-block
  retrieval; journal/undo DTOs; recon spec-compiler; Reviewer enum;
  diary A/B; embeddings before C-metrics say grep failed; merge/
  crossover; a second μ summarizer; multi-agent validator; dsrs/gepars/
  orchestration repos as dependencies (tinyflows additionally GPL-3.0 —
  patterns only); shipping campaign artifacts that fail the
  suite-regression gate.

## 17. Open questions

- A (attempt cap), depth/width caps, ladder thresholds: proposed 2/1/…;
  fitted at S3/S4 from the ledger, recorded as constants with provenance.
- Task subsets pinning T1/T2/T5: pick from first S2 baseline rows.
- Recipe verb surface: `split` only, or also `merge`/`reorder` (both
  currently excluded by expand-only law — revisit only with evidence).
- `context_keys` enforcement depth: namespace filtering in the kernel vs
  copy-into-child-namespace — settle at F10 design time against the
  kernel's actual namespace model.
- Discovery fingerprint normalization (share the mining fingerprint fn).
- Budgets per eval run/campaign; replay publication venue for ARC claims.
- One-in-one-out: S0–S3 build the repo's own gates; S4/S5 is where the
  §1.1 ruling bites — user's call at D75/D77 time.
