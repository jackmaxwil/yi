# Yi self-optimization — plan v4 (prompt-flywheel lineage)

```
status:  PLAN v4 2026-08-29. v1 (naive optimizer) died to self-review; v2
         (lean flywheel) died to the OMP autopsy; v3 (deterministic rules)
         survived but measured with a diary instead of an instrument. v4 is
         the inversion: build the instrument, put Yi's BEHAVIOR under the
         same gate law as Yi's code, and make every improvement — prompt,
         tool, routing, doctrine — a measured commit. Row/D-row numbers are
         placeholders (D73 @ 0.66.0 when written; shared tree — re-read the
         header and last D-row immediately before landing anything).
date:    2026-08-29
sources: research (ref/research/): 2601.04055v1 MPO · 2603.21520v1 MemAPO ·
         2606.04465v1 SePO · 2507.19457v2 GEPA (ICLR 2026) · Factory.ai
         large-tasks PDF · dsrs + gepars (ref/optimizers/, pattern donors,
         NOT Appendix A)
         · omp autopsy (§0; ref/agents/omp, excise respected)
         · Prime Agent: "Prime Agent: A Self-Improving RLM Harness",
         arXiv 2608.23552 (Aug 2026, Prime Intellect + Princeton + MIT);
         primeintellect.ai/blog/prime-agent; docs.arcprize.org/methodology
         (RHAE); arcprize.org/leaderboard + benchlm.ai snapshot (Opus 5
         native 30.2%); github.com/PrimeIntellect-ai/prime-agent
         · benchmarks (ref/benchmarks/, A.12 spans verified 2026-08-22):
         harbor, pier, terminal-bench-2-1, SWE-Atlas, ARC-AGI-3-Agents
         · Yi: ext/{mod,assemble,install,orchestrate,telemetry,project,
         grid}.rs, advisor/{mod,review,digest}.rs, {rules,plan}.rs,
         goal/mod.rs, session/jsonl.rs, types/{entry,advisor,config,plan}.rs,
         permission/catastrophic.rs, tests/request_budget.rs,
         skills/yi/session-mining/SKILL.md, python/yi_runtime (rlm),
         YI_DESIGN §2/§7/§13/§15/§19/A.12, TODOS §E/§F/§I/§J/§N/§P,
         docs/plans/2026-08-29-praxist-lessons.md
```

## Laws (carried through every revision; violations are bugs)

1. **Invariant zero.** The user's prompt is never rewritten, paraphrased, or
   reordered (§7.6 mempalace, P17 retention floor). Optimization touches
   Yi's own assets only.
2. **Deterministic control.** Triggers, admission, eviction, selection, and
   scheduling are exact strings, counters, and thresholds. The LLM writes
   gated prose and generates candidates; it never decides when anything
   fires, persists, ships, or dies.
3. **No unbounded loops.** Nothing is scheduled. Mining and campaigns are
   user-run. CI runs only the zero-API deterministic tier. Real-model
   rollouts are budgeted, deliberate, and ledgered — like dist-binary
   measurements, never like OMP's capture loop.
4. **Cache discipline.** Nothing per-prompt touches trusted blocks (J4/J5,
   0.62.0 breakpoints). Reminders ride the transcript; project text rides
   the yard; no background job rebuilds a prompt mid-session.
5. **Nothing ships unmeasured.** A behavior change is a commit; a commit
   answers to the instrument (§3) and the ratchet (§6).

## 0. Lineage (compressed; the negative space is most of the value)

- **v1 → v2 (self-review):** per-prompt trusted retrieval died to cache
  economics; store/journal DTOs died to `.yi/rules` + D59 promote; the
  recon spec-compiler died to `Task.acceptance` + expand-only `plan.edit`
  (`SHRINK_ERROR`) + `goal::run_check`; GEPA acceptance was unexecutable
  from stored traces — **acceptance requires execution**; sessions already
  partition per cwd; lessons already have a home (project files → yard).
- **v2 → v3 (OMP autopsy, field + code):** OMP ships TTSR, auto-memories,
  auto-skills, and they devolve for mechanical reasons — capture clones the
  whole conversation into an uncached side-agent per firing
  (sdk.ts:1145-1166, fresh cache key each time); write-only stores with no
  outcome ledger, no decay, no retrieval in the local backend
  (memories/index.ts:1332-1414); uncapped per-turn renders — one
  model-written skill description ≈ 16k tokens of every request
  (managed-skills.ts:62-69, system-prompt.md:27-34); buffer-regex fires on
  the *mention*, not the act (export/ttsr.ts:362); model-gated persistence
  with no human gate (manage-skill.ts:40). What does NOT devolve in OMP is
  exactly its deterministic subset — exact-regex triggers, turn counters,
  once-latches, render budgets with clamp (their own mental-models.ts:190
  states the principle: an unbounded block crowds out real context, and a
  curated source cannot be trusted to stay small without enforcement). v3
  rebuilt on that subset: verbatim needles, act-scope, caps, prune-first
  reports, user-run mining from disk.
- **v3 → v4 (benchmark pressure test):** v3's unit of learning (a rule)
  needs repetition to justify itself, goes stale in a fast-moving repo, and
  was to be measured by an unpowered two-week diary A/B. Its harvest is
  project residue that cannot ship to a fresh benchmark environment. The
  fix is not better rules — it is an instrument, and a different unit of
  learning.

## 1. The inversion

Yi's code cannot ship unmeasured: `just check`, shrink-only ratchets,
golden fixtures, red-first regression tests. Yi's *behavior* — the prompt
files sitting modified in this working tree right now, tool descriptions,
routing thresholds, subagent briefs — is the last unmeasured surface in the
repository. v4 closes it.

**The unit of learning is an eval case, not a rule.** A real failure
becomes a deterministic, replayable scenario with a pass condition — seen
red, fixed, kept green forever. One occurrence suffices (rules needed ≥5).
Cases don't rot (a fixed bug's case becomes its regression guard —
nonstationarity becomes the point). Pass/fail is external (no Goodhart on
"user complained less"). Cases are committed to the repo (the per-cwd
session split stops mattering). Every downstream ambition — checks,
campaigns, GEPA, `get_context` — becomes an ordinary tested change riding
an ordinary gate.

## 2. What the tree provides (verified; the instrument is mostly mapped)

| need | mechanism | where |
|---|---|---|
| adapters, span-exact | harbor `BaseInstalledAgent` subclass (~110-line `yi_harbor` per the Pi template), pier subclass + `AgentInstallSpec` one-step musl install, out-of-tree `--agent module:Class`, reward + error-classification contracts | YI_DESIGN A.12 |
| ancillary metrics *already demanded by the harness* | pier `AgentContext` extras: `n_agent_steps`, `peak_context_tokens`, `summarization_count`; TB2.1 leaderboard pass@k schema | A.12 (pier context.py:8-46; leaderboard.yaml:123-141) |
| official ARC scaffold, local | ref/benchmarks/ARC-AGI-3-Agents (agents/, swarm, recorder, templates) | A.12 |
| offline replay substrate | faux provider (scripted streams); J3 cassettes row (record real session → replay with tools stubbed) | §15 / TODOS J3 |
| the ARC-class engine | kernel + `python/yi_runtime` module `rlm`: persistent IPython, context-as-variable, `rlm.run` → host_request → spawn handle (mirrors prime-agent-runtime/src/rlm/__init__.py:148) | crates/kernel · runtime::subagent |
| act-keyed + prompt-keyed delivery, fatigue, promote | D54 rules (`trigger:` literal substrings, `scope: text\|tool:<name>`, `gap: once`), D59 promote with provenance line | rules.rs · advisor/mod.rs:420 |
| spec + executable acceptance | `Task.acceptance`; expand-only `plan.edit` (weakening user-gated); `goal::run_check` exit-0-only with 2,000-char evidence tail; `summary_line` in the advisor digest | plan.rs:17,41 · goal/mod.rs:44 |
| routing + effort levers | `Route` prefilter (weights, thresholds −3/+4, consts 4/5 — hardcoded today, fittable tomorrow); D72 per-model effort ladder carried per turn | ext/orchestrate.rs:70 · D72 |
| orientation chassis | grid ext (fragment today); P3: native `grid_resolve`/`grid_uses`/`grid_scope` + in-process SessionStart survey via `Effect::RegisterTool` | ext/grid.rs · TODOS P3 |
| corpus, immutable raw truth | `~/.yi/sessions/<cwd-slug>/*.jsonl`; `ext_record` rows (route, turn, cache read_ratio) | session/jsonl.rs:25 · ext/telemetry.rs |
| mining skeleton | session-mining skill: kernel-side sweep, defensive parse with skip-count, cluster, coverage line, backtest | skills/yi/session-mining |
| fitted-constants precedent | P4: telemetry → offline fit → `const` baked in | TODOS P4 |
| redaction lexicon | `command_reads_credentials` | permission/catastrophic.rs:154 |

## 3. The instrument

Two tiers, one contract: every tier emits the same metric row (§4).

**Micro-evals (deterministic, zero API, in the gate).** Faux-provider
cassette replays (J3) of pinned scenarios: distilled session failures (§8
pipeline), plus harness behaviors the benchmarks stress — compaction
survival of a mid-task constraint, edit-loop recovery, claim-vs-evidence at
"done", repeated-call rejection, orientation-packet sufficiency. Driven via
`yi ask --json` / headless drive; fast; deterministic; CI-safe (FORGEJO
local-gate law: no API key in any gate).

**Task evals (real rollouts, budgeted, ledgered).** `yi_harbor` + `yi_pier`
adapters (A.12 contracts) over chosen subsets of TB 2.1, SWE-Atlas, and
ARC-AGI-3 (§10). Run deliberately with an explicit USD/token budget; every
run gets a run-id, a config fingerprint (model, effort, prompt-asset
revision), the pier `AgentContext` extras, and a row in the eval ledger
(§6). Timeouts are never retried (pier contract). A failed run is a result,
not a retry-until-green.

**Single-task campaigns (the Factory/GEPA mode).** A campaign pins ONE task
and iterates levers against it — GEPA's inference-time search variant is
the published precedent (NPUEval 4.25% → 30.52% with no held-out set).
Overfit is expected mid-campaign and quarantined at the exit: a campaign
learning ships only as a general asset (prompt diff, tool change, fitted
constant) that holds suite-wide — the regression gate refuses
leaderboard-hack commits. Task-specific tricks stay in the campaign log as
evidence, not product.

## 4. Metrics, levers, targets

**Metric taxonomy** — one row per session and per eval run; names frozen
once in the μ schema (§8). Sources: session JSONL, `ext_record`,
pier/harbor channels.

- **A. Outcome:** task pass; pass@k; first-attempt pass; best-of-n
  selection rate (check-chosen candidate passed); partial credit where
  graded; completion-claim precision (claimed done ∧ grader pass);
  regression count vs baseline; unforced errors (previously-green cases
  now red).
- **B. Spend:** input/output tokens; uncached-input share; cache
  read_ratio; USD; wall-clock; time-to-first-token; provider retries.
- **C. Orientation (the `get_context` axis):** tool calls / tokens / turns
  before the first productive action; orientation sufficiency =
  post-packet new searches for information the packet should have served;
  wasted-read ratio (files read, never used); re-read-after-compaction
  tokens.
- **D. Action economy (the RHAE axis):** total tool calls; per-class mix
  (read/grep/bash/edit/ipython); repeated identical calls; failed-command
  streaks; edit-revert churn; hashline mismatch + noop rates; permission
  asks; **action efficiency = (best-known actions ÷ this-run actions)²,
  capped 1.15** — RHAE's own formula generalized: the per-task best-known
  run is the denominator and the personal leaderboard.
- **E. Delegation:** parent brief bytes vs child-consumed need; child
  result bytes vs parent-used; child tokens per delegated unit; child pass
  rate; fan-out/depth utilization; parent idle-wait.
- **F. Context:** peak context tokens; compaction count; retention-floor
  hits; post-compaction failure correlation; **L2 spill utilization** —
  bulk state parked as kernel variables instead of context (Prime Agent's
  L0 weights / L1 context / L2 REPL / L3 disk hierarchy names the axis;
  P5's context-sentinel gets its metric).
- **G. Long-horizon:** turns; plan coverage (tasks with checks); checks
  run before done; stall/loop detections; steer/interrupt count (the user
  grabbing the wheel — the strongest free negative label).

**Levers** (each names the metrics it moves; every tuned value lands as
data — a prompt-asset commit or a fitted constant with provenance, P4
pattern):

1. Prompt assets — doctrine sections, tool descriptions, orchestrate
   fragment, subagent briefs, the autonomy prompt (§10) → A.
2. Routing — route→effort/model map over the D72 ladder → the A×B
   tradeoff.
3. Retry/selection — N retries on red check only; best-of-n adjudicated by
   the check (**a check is a free judge**: selection without an LLM judge)
   → A up, B bounded.
4. Orientation — `get_context()` composition (§5); grep/read caps (C9)
   → C, then B.
5. Delegation contract — structured brief template + structured result
   schema (F-rows); `rlm.run` depth/fan-out policy → E.
6. Context policy — keep_recent, retention floor, compaction cadence,
   cut-point; kernel-spill policy → F, B.
7. Affordance thresholds — repeated-call N; orchestrate consts
   (TOOL_CALLS_PER_TURN=4, FILES_MATCHED=5, prefilter weights −3/+4) → D.
8. Kernel utilization — when work routes to ipython vs bash vs native
   tools → B, D.
9. The v3 tail — rules/lessons store under the v3 laws → D, marginal.

**Targets (the extreme-ends doctrine).** Hardest tasks expose capability
gaps (missing tools, context strategy); easiest tasks expose pure waste
(every token above minimal is legible); the middle confounds both. So the
campaigns chase the ends:

- **T1 frontier:** pinned hardest TB2.1/SWE-Atlas tasks, currently failing
  → pass at any cost, then ratchet cost down at fixed pass (Factory's own
  sequence: 14× spend to reach 90%, then optimize).
- **T2 efficiency:** pinned easiest tasks, already passing → minimize
  B+C+D at pass = 100%; shrink-only per-task cost ratchet.
- **T3 orientation:** packet sufficiency ≥ target fraction; pre-edit
  orientation cost −X% (X set from T2 baselines).
- **T4 delegation:** brief/result bytes down at equal child pass.
- **T5 ARC (§10):** RHAE on the official scaffold; levers 5/6/8 + the
  autonomy prompt.
- **Universal exit gate:** suite regressions 0, or the campaign's diff
  does not ship.

## 5. `get_context()` — one call, oriented

Intent: today a task's first minutes are a hand-rolled grep/read walk (the
C metrics price it); the target is ONE tool call returning an orientation
packet that makes the walk unnecessary.

- **Input:** the task brief (user prompt or subagent brief), optional
  focus paths.
- **Packet** (layered, budgeted, each layer clamped with named
  truncation):
  1. grid survey slice — module map around the focus (P3 in-process
     survey);
  2. symbol neighborhood — `grid_uses`/`grid_scope`/`grid_resolve` for
     brief-named identifiers;
  3. file skeletons — headers/signatures of the top-k implicated files
     (the hashline read format already renders these);
  4. change heat — git log frequency + recency over the slice;
  5. gates — the repo's own check commands (justfile detection);
  6. prior-issue hits — mining fingerprints (§8) matching the brief:
     "this repo breaks like this, here."
- **Chassis:** P3's `Effect::RegisterTool` at SessionStart (tools freeze
  after SessionStart — the registration window already exists). Layers
  4–6 are cheap adjuncts to P3's planned 1–3.
- **Law compliance:** §1.1 bans embeddings "until grep measurably fails."
  v4 finally makes that condition measurable: the packet is deterministic
  (grid + git + fingerprints); a semantic layer is admissible only when C
  metrics show orientation failing on deterministic retrieval — the eval,
  not taste, relitigates the ban.
- **Tuning:** orientation mining (§8) records which packet layers later
  steps actually used; layer inclusion and budgets become fitted
  constants.

## 6. The behavior ratchet and the eval ledger

- `guardrails/behavior_baseline.json`: the micro-eval case list + pass
  states. Shrink-only failure count, same law as every ratchet; `--update`
  in its own commit, code-first-red order preserved. Runs inside
  `just check` via the faux tier — zero API, deterministic, fast. A new
  gate ⇒ D-row (D76), proven the wall way: neuter the gate, watch a
  known-red case pass, restore.
- `docs/eval-ledger.md` (size-ledger's sibling): one row per task-eval run
  — run-id, suite@rev, config fingerprint, pass, spend, §4 extras. Claims
  about Yi's effectiveness cite ledger rows or they are vibes — a value
  unknown must not be recorded as known (the Praxist thesis, applied to
  ourselves).

## 7. Candidates are commits

The optimizer's and the miner's output is a diff. Git is the store, review
is the gate, the ratchet is the regression net, `git log` is the
provenance chain. For long-horizon consistency, optimization commits carry
a **closed trailer vocabulary** — four trailers, closed the way
check_comments closes comment prefixes (the private-dialect lesson):

```
Tune orchestrate route weights from run 0142

Middle-band prompts misrouted to Complex; T2 campaign, easy suite.

Opt-Run: 0142 suite=tb21-easy@a3f model=opus-5 effort=high
Opt-Delta: pass 48/48=; tokens -21%; wall -18%; regressions 0
Opt-Lever: crates/runtime/src/ext/orchestrate.rs route weights
Opt-Cases: +mined-0093
```

Rules: imperative subject; the why in the body; trailers machine-parseable
so the ledger is reconstructible from `git log` alone; `Opt-Cases` names
eval cases added/retired; baseline and ledger updates ride the follow-up
`--update` commit (baseline-never-with-code law); no assistant co-author
trailers (repo law). Optional later: a guardrail lint for `Opt-*` commits
(closed vocab, unknown trailer rejected) — a row once the template has
survived ~10 real commits.

## 8. Session mining, architected

**Inspirations, one pattern each:**

- **Sentry-class crash pipelines** — fingerprint → issue → lifecycle. A
  failure normalizes to a fingerprint (tool + decisive error line); issues
  carry counts, first/last-seen, linked artifacts; a fingerprint
  reappearing after FIXED auto-flags REGRESSED. Deterministic grouping and
  lifecycle: the anti-write-only-store.
- **ELT / trace analytics** — raw is immutable truth (session JSONL);
  extraction is a *versioned* pure pass (extractor id stamped in every
  derived row; version bump ⇒ re-derive); derived stores are disposable
  caches, never truth.
- **Aviation safety (ASRS/blameless)** — human-gated reports, fixed
  taxonomy, the mandatory coverage line (`N scanned, M skipped (reason),
  DATE..DATE`), uncertainty labeled rather than dropped.
- **Prime Agent's Continual Harness** — versioned cross-trajectory state
  (notes, memories, skills, subagent specs): direction validated at scale,
  adopted with one inversion — their auto-persist becomes our human gate
  (their paper itself declines to attribute wins to it; OMP shows the auto
  version's failure mode).
- **TRACE** — corrections compiled into checks beat corrections stored as
  prose; the reason cases and checks outrank rules.
- **GEPA/MemAPO/SePO** — reflection record shape `{Inputs, Generated
  Outputs, Feedback}`, 1:1 failure/success balance, verify-before-update.

**What we mine (extraction schema v1; one pass per session file; kernel
python; defensive parse with skip-count):**

1. μ row — the §4 metrics computable from the trace (A minus grader
   fields, B, C, D, E, F, G).
2. Failure events — `{tool, fingerprint, args_hash, resolved_in_session,
   resolution_action}`.
3. Friction events — permission asks, retries, repeated-call rejections,
   reverts/`/undo`, steer/interrupts, correction turns (verbatim; the §7.6
   lexicon via `advisor::digest::directives` — reuse, don't fork).
4. Orientation trace — pre-first-edit reads/searches, and which were later
   used (feeds §5 layer tuning).
5. Delegation exchanges — brief/result sizes, child spend, child outcome.
6. Wins — clean completions (reflection balance; positive exemplars).

**Derived stores** (`.yi/mining/`, gitignored, re-derivable): `mu.jsonl` ·
`issues.jsonl` (fingerprint ledger, lifecycle NEW → CASED → FIXED(commit)
→ REGRESSED → RETIRED; transitions are data events, never model judgment)
· `orientation.jsonl` · `delegation.jsonl`. Redaction (§9) runs at
extraction, so secrets never reach a derived store.

**Outcomes driven, ranked (the v3 inversion made explicit):**

1. **Eval case** — the failure distilled to a committed fixture (CASED);
   drives the ratchet.
2. **Tool/affordance change** — a TODOS row with the issue's numbers
   attached (the mining skill always listed this output; it is the
   highest-value one).
3. **Doctrine/prompt edit** — ratchet-gated commit.
4. **Fitted constant** — thresholds/weights from data (P4 pattern).
5. **Trigger rule / lesson** — the zero-code tail, v3 laws intact
   (verbatim needles, act-scope, ≤400-byte bodies, ≤12 rules,
   prune-first reports, ≥1 deletion per addition at cap).

User outcome: one report — coverage line, issue board with lifecycle,
prune list first, proposals ranked by the list above with numbers
attached. Harness outcomes: cases, constants, rows. Nothing lands without
the user.

## 9. Redaction (unchanged law, wider duty)

Applied at extraction and to every artifact, case, ledger row, and
reflection prompt: credential-store path lines dropped
(`command_reads_credentials` lexicon — one vocabulary, second use);
`authorization|bearer|api[-_]?key|token|secret|password` lines masked;
`NAME=value` with ≥16-char mixed values and bare ≥32-char high-entropy
tokens masked (SHAs/ulids get caught — acceptable, noted in the report);
absolute `$HOME` → `~`. A planted-fake-secret fixture rides the mining
skill as a self-check; plants must never appear in any output.

## 10. ARC-AGI and the kernel (researched 2026-08-29)

Findings, confidence marked. Prime Agent reports **95.5% ARC-AGI-3 RHAE
Best@1** (runs 95.0/95.2/95.5; 99.97% Best@3, 183/183 levels) with Claude
Opus 5 — *self-reported with a published action replay, not an
ARC-Prize-verified leaderboard entry*; the official leaderboard's top base
model is Opus 5 native at 30.2%, and other heavy harnesses cluster high
(NVIDIA AVO claims 100%, Schema ~99%). RHAE = (human actions ÷ agent
actions)², capped 1.15 — squared action efficiency (verified,
docs.arcprize.org). Mechanism per the paper: **no ARC-specific workflow**
— a persistent IPython REPL holding game state as variables, exploration
and verification run as code, `rlm.run` recursive subagents returning
handles, a PRO-LONG-style autonomous prompt. The local prime-agent clone
contains zero ARC code (verified) — the mechanism is the harness itself.

**Implication: Yi inherited the capability class.** The kernel +
`python/yi_runtime` `rlm` package mirror prime-agent-runtime's
`rlm.run` → host_request → spawn-handle loop by construction (the phase-4
port). ARC therefore moves from "out of scope" (my v3 error) to
"measurable, with named levers": the official ARC-AGI-3-Agents scaffold is
already cloned; the levers are §4's 5/6/8 (delegation policy, kernel-spill
policy, kernel utilization) plus the autonomy prompt as an optimizable
asset. RHAE's formula is adopted repo-wide as the D-axis efficiency
metric — the benchmark's own math rewards exactly the extreme-ends
efficiency campaign.

Honesty rails: nothing is claimed until run on the official scaffold; Yi
publishes replays (the recorder ships in the scaffold) or claims nothing;
the 95.5% also rode Opus 5 plus a prompt, neither of which is architecture
— expect the gap between "inherits the class" and "reproduces the number"
to be real work, and let the first runs turn missing kernel-manager pieces
(fork server, state snapshot, watchdog) into K-rows with numbers.

## 11. Online deterministic layer

`Task.check: Option<String>` (additive, §19 rule 3; schemas.lock + golden
fixture): `plan.update(done)` runs it via [`goal::run_check`]; red refuses
the transition and returns the evidence tail. The advisor digest gains the
count of done-claims on check-less tasks plus one sentence in
`ADVISOR_SYSTEM_PROMPT` (no Reviewer enum — one reviewer, one prompt).
D50-clean: completion enforcement the model opted into — the goal module's
existing contract, per task. On top, the two check-adjudicated test-time
policies (§4 lever 3): bounded retry-on-red and best-of-n-selected-by-
check — each measured on the instrument before it default-enables.

## 12. Stages, gates, kill criteria (instrument first)

- **S0 — mining foundation (skill + kernel python; no Rust).** Extraction
  schema v1, signal census (advisory-derived fields are usually absent —
  count first), derived stores, issue board, redaction plants, coverage
  line, first distilled failure scenarios. Kill: the corpus yields no
  distillable failures — then Yi's problem is not learnable from its
  history and only S2 benchmarks can steer.
- **S1 — micro tier + ratchet (J3 + D76).** Cassette record/replay over
  faux; `behavior_baseline.json` wired into `just check`; the
  neutered-gate proof. First cases: S0's scenarios + §3's harness
  behaviors. Kill: cassettes prove nondeterministic across two runs — fix
  that before scaling anything.
- **S2 — adapters + ledger (J1/J2, shaped by this plan).** `yi_harbor`,
  `yi_pier`, metric emission incl. pier extras, `docs/eval-ledger.md`,
  first baseline rows on TB2.1 + SWE-Atlas subsets + one ARC-AGI-3 run.
  No kill — this stage only reveals; its numbers steer everything after.
- **S3 — online layer (N11).** Task.check, retry-on-red, best-of-by-check,
  route→effort map — each landed only with an S2 delta and zero suite
  regressions.
- **S4 — campaigns + get_context.** T1/T2 extreme-ends campaigns; the §5
  packet on the P3 chassis; orientation metrics close the loop; the
  embeddings ban relitigated by C metrics only.
- **S5 — GEPA proper (D75).** Round-robin module optimization over the
  instrument; candidates-as-commits under §7; reflection budget ≤ 92
  calls/run (GEPA Table 4; tunable). Kill: the frontier-headroom test —
  if one budgeted pass moves nothing on S2 baselines, record that in the
  ledger and stop.

## 13. Guardrail and ratchet impact

| dimension | S0 | S1 | S2 | S3+ | note |
|---|---|---|---|---|---|
| deps (direct/transitive) | 0 | 0 | 0 | 0 | adapters are python under evals/ (J2's stated shape); dsrs/gepars stay pattern donors (banned crates welded in; 483/229 locks) |
| YI_* vars / config keys / CLI verbs | 0 | 0 | 0 | 0 until a run needs a knob — then the row first | env cap 40 untouched |
| src LOC | 0 | cassette + gate glue | 0 (evals/ workspace has its own test-LOC budget) | ≈ +100 (N11) + P3 adjuncts | ratchet `--update` own-commit law |
| prompts/.md | 0 | 0 | 0 | +1 sentence (advisor) · reflect.md at S5 | duplication budget 0 |
| new gates | — | behavior_baseline (D76) | — | — | proven by neutering |
| request-prefix bytes | 0 | 0 | 0 | 0 | law 4 |
| API spend in CI | 0 | 0 | 0 | 0 | task evals user-run, budgeted, ledgered |

## 14. Rows and D-rows (renumbered 2026-08-29 against the live file:
`P8`-`P11` and `A12`/`F7`/`O10` were taken by the session-01a04c94 batch)

- `P12` mining foundation · M · skills/yi/session-mining + kernel python —
  S0 as §8. done: report over the real corpus; coverage line; plants
  absent; ≥1 failure distilled to a replayable scenario spec.
- `J3'` (shape the existing J3 row): cassettes serve the micro tier; case
  format = cassette + pass condition. done: one mined case replays twice
  with byte-identical verdicts.
- `J11` behavior ratchet · S · scripts/guardrails + baselines (needs
  D-row D76). done: a red case blocks `just check`; neutered-gate proof;
  baseline `--update` in its own commit.
- `J1'/J2'` (shape the existing J1/J2 rows): adapters emit §4 metrics
  incl. pier extras; eval-ledger.md; first TB2.1 / SWE-Atlas / ARC rows.
  done: a ledger row per suite with config fingerprint.
- `N11` task checks · M · types/plan.rs + runtime/{plan,advisor} (needs
  D-row D74) — §11. done: red-check refusal, neutered proof, golden
  fixture, older-reader passthrough.
- `P13` get_context · M · rides P3 (grid SDK) + orientation mining — §5.
  done: packet tool registered at SessionStart; orientation-cost delta
  measured on T2 tasks.
- Campaign and GEPA rows land at S4/S5 with D75.

D-row drafts (texts only; claim numbers at land after re-reading the
header — 0.35.0/D55 and 0.38.0/D57 were both lost to this):

- **D74 — executable acceptance on plan tasks.** `Task.check` additive;
  `plan.update(done)` runs it via the goal check runner; red refuses with
  the evidence tail; the advisor digest counts check-less done-claims.
  Why: Factory's 36→90% result that self-authored completion criteria
  collapse; deterministic enforcement the model opted into (D50-clean);
  the plan module's expand-only law supplies the freeze. Reversible-via:
  drop the field read + header count; the field survives as unknown data.
- **D75 — instrument-driven optimization (campaigns + GEPA), parked until
  S2 baselines exist.** Candidates are commits under the §7 template; exit
  gate = suite regressions 0; extreme-ends campaign doctrine.
  Reversible-via: additive tooling; no data stranded.
- **D76 — behavior baseline gate.** A shrink-only micro-eval ratchet in
  `just check`, faux tier only. Why: prompts and behavior are the last
  unmeasured surface; the code-gate law extends to them. Reversible-via:
  remove the gate call + baseline file; cases remain ordinary tests.

## 15. Not building (accumulated; each with its killer)

- Live prompt rewriting/paraphrase; per-prompt GEPA (invariant zero;
  negative-EV: SePO baselines, MPO/TextGrad, GEPA's greedy ablation).
- Scheduled/heartbeat anything (OMP capture: full-context clone, fresh
  cache key per firing, no budget); LLM-judged firing / persistence /
  eviction (law 2); TTSR-style stream interrupts (mention/act confusion is
  structural to buffer-regex); uncapped per-item renders and whole-store
  prompt dumps (OMP's 16k-token skill, 5k memory dump); write-only stores
  (no ledger, no prune).
- Prime-style auto-persisted continual state (their own paper won't claim
  the credit; the human gate stays).
- Per-prompt trusted-block retrieval (cache law); journal/undo/store DTOs;
  recon spec-compiler + TaskSpec entry (plan/goal own it); a Reviewer job
  enum; diary A/B as evidence (the instrument replaced it).
- Embeddings/semantic search until C metrics prove deterministic
  orientation fails (§1.1's own condition, finally measurable);
  merge/crossover (no-op at this module count; hurt the smaller model in
  GEPA); a second μ summarizer; a multi-agent validator (the advisor is
  singular, §7.5); dsrs/gepars as dependencies; shipping any campaign
  artifact that fails the suite-regression gate (leaderboard hacks stay in
  the log).

## 16. Open questions

- Task subsets: which TB2.1/SWE-Atlas tasks pin T1 (hardest) and T2
  (easiest); which ARC level slice pins T5. Pick from the first S2
  baseline rows, not a priori.
- Budgets: USD/token cap per task-eval run and per campaign; set at the
  first S2 run, recorded in the ledger header.
- Case format: cassette granularity (full session vs turn-slice) — settle
  in J3' against a real mined failure.
- One-in-one-out: S0–S3 build the repo's own gates, not §1.1 features;
  S4/S5 is where the ruling bites — user's call at D75 time.
- Commit-lint guardrail for `Opt-*` trailers: only after the template
  survives ~10 real commits.
- Replay publication for ARC claims: where replays live (repo? ledger
  artifact?) — decide at the first ARC run.
