# Prompt flywheel — architecture and implementation plan, v3

```
status:  PLAN v3 2026-08-29. v1 was cut down by an adversarial self-review
         (§0); v2 was cut down again by the OMP autopsy (§1) — field report:
         OMP's TTSR, auto-memories and auto-skills "work initially but very
         quickly devolve, become noisy and context wasting". v3's law: every
         loop is driven by deterministic signals; the LLM writes prose inside
         a human gate and controls nothing. Row ids and D-row numbers are
         placeholders — ARCHITECTURE.md held D73 at 0.66.0 when written;
         re-read the header and last D-row immediately before landing.
date:    2026-08-29
sources: research (ref/research/): 2601.04055v1 MPO · 2603.21520v1 MemAPO ·
         2606.04465v1 SePO · 2507.19457v2 GEPA (ICLR 2026) · Factory.ai
         large-tasks PDF · pattern donors (ref/optimizers/ dsrs, gepars —
         NOT in Appendix A; ports need an A-entry first)
         · omp autopsy (ref/agents/omp, excise list respected):
         export/ttsr.ts, ttsr-coordinator.ts, prompts/system/{ttsr-interrupt,
         ttsr-tool-reminder,autolearn-guidance,system-prompt}.md,
         memories/index.ts, autolearn/{controller,managed-skills}.ts,
         tools/{learn,manage-skill}.ts, hindsight/state.ts, mnemopi/state.ts,
         sdk.ts, mental-models.ts, docs/ttsr-injection-lifecycle.md
         · Yi: crates/runtime/src/ext/{mod,assemble,install,orchestrate,
         telemetry,project}.rs, advisor/{mod,review,digest}.rs,
         {rules,plan}.rs, goal/mod.rs, crates/session/src/jsonl.rs,
         crates/types/src/{entry,advisor,config,plan}.rs,
         crates/permission/src/catastrophic.rs, tests/request_budget.rs,
         skills/yi/session-mining/SKILL.md, docs/YI_DESIGN.md §2/§7/§13/§19,
         docs/TODOS.md, docs/plans/2026-08-29-praxist-lessons.md
```

**Invariant zero.** The user's prompt is never rewritten, paraphrased, or
reordered; it reaches the model byte-verbatim (§7.6 mempalace rule, P17
retention floor). The flywheel appends only through mechanisms that already
exist and only improves Yi's *own* prompt assets, offline. Research evidence
(v1/v2, unchanged): zero-feedback rewrites are negative-EV (SePO, MPO/
TextGrad, GEPA greedy ablation); MemAPO appends and never paraphrases; GEPA
is a category error per-prompt and literal over a corpus; Factory's gains
came from an independent executable standard, not better prose.

**Division-of-labor law (v3).** Deterministic signals drive every loop:
triggers are exact substrings and counters; admission and eviction are
thresholds over measured numbers; caps are enforced at the render site. The
LLM appears in exactly three places, each inside a human gate: writing the
prose body of a proposed rule/lesson during a *user-run* mining pass, the
existing advisor reviewer (already model-role-gated, D28), and the parked v4
search. The LLM never decides when anything fires, persists, or dies.

## 0. What the self-review changed (v1 → v2, kept for the record)

| v1 design | tree fact | consequence |
|---|---|---|
| per-prompt retrieval attaching trusted fragments | four-breakpoint cache layout, 1h retention (0.62.0); J4 byte-identical prefix; only the yard varies | anything always-on must be session-stable; per-prompt trusted mutation is cache-hostile and dead |
| TemplateStore + journal + undo + augmenter extension | `RuleScope::Text`/`Tool` rules with literal `trigger:` substrings, `gap: once`, project-shadows-global, malformed-skipped-with-reason; `/advisor promote` (D59) writes provenance-lined rule files and arms them live | the store IS `.yi/rules`; journal/fold/undo/extension dead |
| GEPA acceptance "scored from stored μ only" | a candidate's effect on a session that ran without it is unmeasurable; GEPA/gepars/dsrs execute the minibatch | reflection reads traces; **acceptance requires execution** — search parks behind the J1 evals workspace (v5) |
| TaskSpec entry + recon compiler | `Task.acceptance` exists; `plan.edit` expand-only, weakening user-gated (`SHRINK_ERROR`); `summary_line` already feeds the advisor digest; `goal::run_check` runs checks exit-0-only with evidence tails | lane 2 collapses to one additive `Task.check` field + a gate at `plan.update(done)` (v4) |
| project scoping open | sessions already partition per cwd (`jsonl.rs::session_directory_name`) | corpus is `~/.yi/sessions/<cwd-slug>/` |
| new fragment slot for lessons | AGENTS.md/CLAUDE.md already load into the trust-gated yard (`ext/project.rs`) | lessons are a project-file section; zero code |

## 1. OMP autopsy — why the naive version devolves (verified in code)

OMP ships all three halves of this feature and the user has watched them
decay in real use. The code says why, and none of the reasons is "the idea
is wrong" — every one is a missing deterministic bound:

| OMP subsystem | what devolves | code evidence |
|---|---|---|
| TTSR (Time-Traveling Stream Rules: regex over the model's own stream, abort + retry with the full rule body injected) | the *trigger substrate*: regex runs over a monotonically growing buffer, so a rule fires on the **mention** of a pattern, not the act; scope defaults to text + all tools; injected body is the whole rule markdown, uncapped (~300–700 tokens per fire); no per-session injection budget; a failed delivery is still marked delivered | export/ttsr.ts:362 (append, never window), :65-70 (default scope), ttsr-coordinator.ts:163-176 (uncapped body), docs/ttsr-injection-lifecycle.md:169 |
| auto-memories (`learn` tool + session-start pipeline) | write-only store: no verification before storing, no outcome ledger, no decay, dedup is exact-string only, and the `local` backend has **no retrieval at all** — the whole file dumps into the system prompt every session (clamped at 5k tokens, so at scale it is 5k tokens of mostly-stale bullets every turn) | memories/index.ts:1332-1358, :1380, :1400-1414, :219-221 |
| auto-skills | model-judged creation with no human gate, no count cap in the prompt listing, no length cap on the per-turn description (one model-written skill can put ~16k tokens into every request), no versioning, no pruning, no usage counter | tools/manage-skill.ts:40, prompts/system/system-prompt.md:27-34, autolearn/managed-skills.ts:62-69 |
| scheduling (auto-learn capture, TTL refresh) | **the token drain, precisely**: capture clones the *entire conversation* into a fresh side-agent with a *new cache key per capture*, re-billing the full context uncached, triggered by a bare counter (≥5 tool calls at agent_end) with no cooldown, cap, or budget; a background TTL job rebuilds the system prompt mid-session, invalidating the provider cache prefix | sdk.ts:1136-1203 (:1145,1166 fresh cache key), autolearn/controller.ts:113-136, hindsight/state.ts:498-511 |

What in OMP does *not* devolve, and what it has in common: exact-regex and
turn-counter triggers, the once-per-session recall latch, `minToolCalls`,
render budgets with clamp-to-zero, SQLite leases (export/ttsr.ts:94-99,
hindsight/state.ts:419, memories/index.ts:219-221). All deterministic. OMP's
own mental-models.ts:190-197 states the principle: an unbounded block crowds
out real context, and a curated source cannot be trusted to stay small
without enforcement.

Countermeasures now load-bearing in this plan: (1) mine from **disk**, never
by cloning live context into an uncached side-turn; (2) no scheduled
reflection at all — mining is user-run; (3) every store has a cap, an
outcome ledger, and an evidence-driven prune path; (4) triggers match the
**act** (tool arguments) not the mention, with text-scope reserved for
prompt-time strategy hints; (5) per-item rendered-size caps at the render
site; (6) nothing persists or dies by model judgment.

## 2. What the tree provides (verified)

| need | mechanism | file |
|---|---|---|
| act-keyed delivery | `RuleScope::Tool(name)`/`AnyTool` rules match tool use; `Gate` denies first attempt, informed retry passes | crates/runtime/src/rules.rs:12 |
| prompt-keyed delivery | `RuleScope::Text`, literal `trigger:` substrings | rules.rs:107 |
| per-rule fatigue | `RuleGap::Once` (one fire per session) / `AfterTurns(n)` — OMP's proven `repeatMode`/gap counters, already here | rules.rs:19 |
| promote precedent | advisor promote writes `.yi/rules/<slug>.md` with provenance, re-parses, arms live (D59/I1) | advisor/mod.rs:420,556 |
| always-on lessons | AGENTS.md/CLAUDE.md → yard, nonce-fenced, hash-pinned trust | ext/project.rs:8 |
| spec with acceptance | `Task { title, acceptance, deps, state }`; expand-only; weakening user-gated | plan.rs:17 |
| executable check + evidence | `goal::run_check`: exit 0 only; Err carries check + exit + 2,000-char tail | goal/mod.rs:44 |
| advisor sees the plan | `summary_line(plan)` built for the digest header | plan.rs:41 |
| corpus, per project, on disk | `~/.yi/sessions/<cwd-slug>/*.jsonl` | session/jsonl.rs:25 |
| sweep skeleton | session-mining skill: kernel-side sweep (bulk never enters context), defensive parse, cluster, coverage line, backtest | skills/yi/session-mining/SKILL.md |
| outcome telemetry | `Effect::Record` → `ext_record` rows (`route`, `turn`, `cache` incl. read_ratio); rule fires are visible in the transcript as delivered reminder text | ext/telemetry.rs |
| credential lexicon | `command_reads_credentials` patterns | permission/catastrophic.rs:154 |

## 3. Architecture

```
~/.yi/sessions/<project>/*.jsonl            (disk — the only mining input)
      │  USER RUNS mining (session-mining skill; sweep in kernel python;
      │  no context clone, no side-agent, no fresh cache key — the OMP
      │  drain is structurally impossible from disk)
      ▼
mining report (redacted, §6): μ census · failure clusters · novelty check
      · PRUNE LIST FIRST (outcome-ledger-driven) · then proposals, each
      with derived trigger + backtest numbers
      │
      ▼
USER lands artifacts (nothing persists or dies without this):
  error rules  → .yi/rules/<name>.md  scope tool:<name> (match the act),
                 gap: once, body ≤ 400 bytes, provenance line
  strategy     → scope:text rule (prompt-keyed hint) or AGENTS.md
  lessons        "## Lessons" ≤ 1,500 bytes total (yard, session-stable)
      │
      ▼
sessions run · rules fire as one-line transcript reminders (prefix never
moves) · fires and outcomes accrue in the JSONL for the next mining pass
      │
      └───────────── next USER-RUN pass measures and prunes ──────────────┘

v4: Task.check — executable acceptance at plan.update(done)   (first Rust)
v5: GEPA search over the J1 evals workspace                    (parked)
```

Cache law (v2, now with the OMP contrast): nothing per-prompt touches
trusted blocks (J4/J5, 0.62.0 breakpoints). Rules deliver in-transcript;
lessons ride the yard; there are no mid-session prompt rebuilds — the OMP
TTL-refresh bug class (hindsight/state.ts:498-511) cannot occur because no
background job here ever touches the prompt.

The wall, stated honestly (unchanged): checks are "not pushed into primary
context by default", enforced by the v4 gate and the reviewer's evidence
demand — Factory's hard wall does not exist in one process with a `read`
tool.

## 4. The rule lifecycle, fully specified (the v2 gap)

Everything below is a number or an exact string; the LLM appears once,
marked.

**Birth (mining pass, user-run).**
- Cluster key: `(tool name, normalized error line)` — the skill's existing
  key. A cluster is *actionable* at ≥ 5 occurrences across ≥ 3 sessions
  (tunable; recurrence across sessions is what distinguishes a lesson from
  an incident).
- Trigger needles are **derived verbatim from the cluster's data**: the
  exact failing command prefix or error substring (e.g. `git commit -m`
  with a backtick in the argument), never LLM-invented keywords. The report
  prints needle + provenance sessions.
- Scope defaults to `tool:<name>` — the rule matches the *act* (tool
  arguments), the OMP mention/act fix. `scope: text` is reserved for
  strategy hints where prompt wording *is* the signal.
- Backtest, deterministic: replay the needle over the corpus. Admission
  thresholds: fires in ≥ 3 failure-cluster sessions AND in ≤ 1 session
  without the failure (precision ≥ 0.75 on history). Both numbers print in
  the report; below threshold, the proposal is not emitted.
- [LLM, human-gated] The rule's prose body is drafted by the model from the
  cluster evidence, ≤ 400 bytes rendered (the OMP uncapped-body fix),
  redacted (§6). The user lands the file or doesn't.

**Life.**
- Caps, enforced by the mining report refusing to propose past them:
  ≤ 12 active rules per project (tunable), body ≤ 400 bytes, lessons block
  ≤ 1,500 bytes. Worst-case injection per session is bounded by
  `gap: once` × 12 rules ≈ 12 one-liners — legible, and an order of
  magnitude under OMP's single-skill worst case.
- Outcome ledger, deterministic, computed at the next mining pass from the
  transcripts (a fired rule is its delivered reminder text in the session
  JSONL): per rule — `fires`, `post_fire_failure_rate` (the cluster's error
  signature still occurred after the fire, same session), `zero_fire_streak`
  (sessions since last fire). If body-text matching proves ambiguous in
  practice, the one code concession is a `Record{key:"rule_fired"}` row at
  delivery (~10 lines) — deferred until ambiguity is demonstrated.

**Death (the OMP write-only-store fix; shrink-biased like every Yi ratchet).**
- `post_fire_failure_rate ≥ 0.6` over ≥ 5 fires → propose delete or rewrite
  (the rule fires and doesn't help).
- `zero_fire_streak ≥ 30` sessions → propose delete (dead trigger).
- Store at cap → the report must propose ≥ 1 deletion per addition.
- The **prune list is the report's first section**, before proposals. The
  user lands deletions the same way as additions. Nothing is auto-deleted.

**Mining trigger.** User-run, full stop. No heartbeat, no schedule, no
session-end counter nudging — OMP's capture-on-counter is the drain
(§1 row 4), and even a zero-token nudge is a standing surface that earns
nothing a user noticing their own friction doesn't. Cost shape of one pass:
kernel-side sweep over disk files (no model tokens for the bulk), one
in-session drafting step over cluster tables only.

## 5. Stages, gates, kill criteria

**v0 — measure (skill edit only).** Extend skills/yi/session-mining with:
signal census (advisory-derived μ fields are absent unless a model role
named the reviewer — count before trusting), the μ table (§below) as one
JSON row per session, the novelty check (proposals diffed against
doctrine.md, har-core.md, identity.md, orchestrate.md, existing `.yi/rules`
— a rule the system prompt already states is spend, not learning), the §4
backtest with its confound stated (fired sessions are the harder sessions;
deltas are direction, not proof), §6 redaction applied to the report itself,
and the §4 lifecycle numbers (admission thresholds, prune rules) as the
report's fixed skeleton.
Kill: correction-classifier precision < 0.8 on hand-labeled n ≥ 50, or no
*novel* actionable cluster. Then the flywheel dies here, documented.

μ row (python emits; v5's Rust, if ever, freezes the same shape in
yi-types then): `muTestsGreen: bool|null` (last build/test bash result:
error flag + `exit N` tail; field shapes settled against a real fixture),
`muCorrectionTurns: u32` (§7.6 constraint lexicon, the
`advisor::digest::directives` table — reuse, don't fork; classifier
validated by hand-label first), `muRevertOps` (checkpoint/rewind entries +
re-edit-after-failing-bash), `muTurnsToFirstGreenEdit`, `muAbandoned`,
`muClaimUnbacked` (census decides if the column is real), `muTokens`,
`muCacheReadRatio` (the cost half of every comparison).

**v1 — land and measure prospectively (no code).** User lands the best v0
artifacts. Two-week toggle (`.yi/rules` renamed aside + lessons block
in/out, alternating weekly). Compare `muCorrectionTurns`,
turns-to-completion, **and the cost pair** (`muTokens`, cache read_ratio) —
metrics lie in pairs. Dozens of heterogeneous sessions cannot reach
significance; the pre-declared bar is directional benefit with no cost
regression.
Kill: flat or cost-negative → artifacts stay (they are the user's words),
v2+ stops.

**v2 — the prune loop (no code).** Second and later user-run passes: the
outcome ledger drives the prune list; the store provably shrinks when
evidence says shrink. This stage exists to demonstrate the property OMP
lacks — a store that gets *smaller* on data.

**v3 — (retired; was scheduling).** Killed by §1 row 4. Recorded here so it
stays dead.

**v4 — checks on plan tasks (~100 lines + tests; needs a D-row).** One
additive field and one deterministic gate:

```rust
// yi-types/src/plan.rs — additive (§19 rule 3), schemas.lock + fixture.
pub struct Task {
    // ...existing fields...
    /// One command whose exit code establishes the acceptance; None is an
    /// honest hole the advisor may name at a done-claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}
```

`plan.update(state=done)` on a task with a check runs it through
[`goal::run_check`] (same crate; exit-0-only, 2,000-char evidence tail). A
red check refuses the transition and returns the evidence — the goal
module's existing completion contract, now per task. The advisor digest
header adds the count of done-claims on check-less tasks plus one sentence
in `ADVISOR_SYSTEM_PROMPT`. No `Reviewer` job enum (one reviewer, one
prompt). Freeze semantics already exist (`plan.edit` expand-only). D50
permits all of it: this is completion *enforcement* the model opted into,
not deterministic advice.
Kill: users route around checks (tasks systematically declared without
them) → keep the field, drop the advisor sentence.

**v5 — real search (needs J1/J2/J3; own D-row; parked).** GEPA over the
evals workspace, where acceptance can execute: two-phase (minibatch strict
`σ' > σ`, ε = 0 until noise data argues; SePO's ε is admission *leniency*
`σ' ≥ σ − ε`, not a higher bar), frequency-weighted Pareto draw (dominance
pruning skipped under 20 candidates), rollout budget as a value the loop
breaks on, `{Inputs, Generated Outputs, Feedback}` reflection records with
1:1 failure/success balance, GEPA App. C meta-prompt adapted once, seeded
xorshift64* for the banned-`rand` draw (seed forced nonzero — zero is a
fixed point — via the `session_nonce` idiom, logged for replay), keyword
matching tokenized like `orchestrate::word_hits` (substring `contains`
matches "port" in "important"), MemAPO's UPDATE gate made real (a revised
asset must still win its stored eval cases). Reflection ceiling per run: 92
calls (GEPA Table 4; tunable). Human promote still gates persistence.

## 6. Redaction

Applied to every string that leaves a session file — report, rule text,
lessons, any reflection prompt. v0–v2 enforce as numbered skill steps
(human-gated landing is the backstop); v5's automation moves it into code.

1. Drop lines naming a credential-store path (`command_reads_credentials`
   lexicon — one vocabulary, second use; the N10 argument).
2. Mask lines matching `authorization|bearer|api[-_]?key|token|secret|password`
   (case-insensitive).
3. Mask `NAME=value` with value ≥ 16 chars mixed alpha/digit no spaces;
   bare ≥ 32-char high-entropy tokens. Git SHAs/ulids get caught —
   acceptable, noted in the report when it happens.
4. Absolute `$HOME` prefixes → `~`.

A planted-fake-secret fixture (never a real key) rides the v0 skill as a
self-check; the report must not contain the plants.

## 7. Artifact formats (no new schemas before v4)

Rule file — exactly the D54 shape `read_rule` parses, provenance-lined like
`rule_markdown` output, act-scoped per §4:

```markdown
---
trigger: git commit -m, git commit --message
scope: tool:bash
mode: remind
gap: once
---
Commit messages containing backticks or $( go through `git commit -F -`
with a quoted heredoc; zsh command-substitutes inside double quotes.

(mined from sessions 01a04c94, 3f2201aa; backtest 4/4 failure sessions,
0 benign; edit or delete this file to change it)
```

Lessons — `## Lessons` in AGENTS.md (this repo: via .ruler/ + apply), one
sentence per lesson, ≤ 1,500 bytes total, provenance in a trailing comment.

Mining report skeleton, in order: coverage line (`N scanned, M skipped
(reason), DATE..DATE` — never skipped) → μ census → **prune list with
ledger numbers** → proposals (artifact verbatim + needle provenance +
backtest numbers + novelty verdict) → the thresholds used.

yi-types additions: none until v4 (`Task.check`), then schemas.lock +
golden fixture in the same commit.

## 8. Tests

v0 (the measurement must be trustworthy first): hand-label ≥ 50 turns for
the correction classifier, report precision/recall (< 0.8 kills);
determinism (sweep twice, reports diff empty); time-split honesty (mine
older 80%, hold out newest 20%, drift confound noted); redaction plants
absent; coverage line present; **lifecycle dry-run** — the report's prune
and admission sections compute from a synthetic corpus fixture with known
counts, so the thresholds are exercised before they ever judge real rules.

v4 (doctrine-compliant; each test names the consumer-visible failure):
`Task.check` round-trips through a real session file and an older reader
passes it through (§19 rules 3/6; golden fixture, never edited);
`plan.update(done)` on a red check refuses and the model sees the evidence
tail — proven by neutering the gate and watching the wrongly-green
transition; a done-claim on a check-less task surfaces in the digest header
count (faux provider scripting the reviewer); any fix-shaped test runs red
against unfixed code first.

v5 tests ride its own D-row.

## 9. Guardrail and ratchet impact

| dimension | v0–v2 | v4 | note |
|---|---|---|---|
| direct/transitive deps | 0 | 0 | dsrs/gepars pattern donors only — both dependency-disqualified (banned crates welded in; 483/229-package locks) |
| new crates / modules / config keys / YI_* vars / CLI verbs | 0 | 0 | |
| src LOC | 0 | ≈ +100 | plus the deferred ~10-line `rule_fired` Record only if body-match attribution proves ambiguous |
| prompts/.md | 0 | +1 sentence in an existing const | reflect.md deferred to v5 |
| request-prefix bytes | 0 | 0 | rules deliver in-transcript; lessons ride the yard; J5 measures a fixed test scenario |
| schemas.lock | 0 | +1 type touched | fixture same commit |
| per-session injection (not a CI gate; a design bound) | ≤ 12 one-line reminders + ≤ 1,500-byte yard block | — | vs OMP: 5k-token memory dump every turn, 16k-token skill descriptions, 300–700-token stream interrupts |

## 10. Draft rows and D-rows

Rows (renumbered 2026-08-29 against the live file: the `P8`/`P9` this draft
held were taken by the session-01a04c94 batch, exactly the collision the
header warns about. TODOS is worked in file order, so placement is
sequencing):

- `P12` flywheel v0 sweep · S · skills/yi/session-mining — census, μ rows,
  novelty check, §4 lifecycle skeleton (admission/prune thresholds),
  redaction steps, artifact formats.
  done: report over the real per-project corpus; coverage line; classifier
  precision/recall stated; plants absent; lifecycle dry-run fixture green;
  the kill question answered either way.
- `P13` flywheel v1/v2 measurement · S · no code — land chosen artifacts;
  two-week toggle; benefit + cost pairs; second pass exercises the prune
  list on real ledger numbers.
  done: a written keep/kill verdict with the numbers, and at least one
  evidence-driven prune executed or explicitly declined.
- `N11` task checks · M · crates/types/src/plan.rs +
  crates/runtime/src/{plan.rs, advisor/} (needs D-row; N owns the plan
  system, D52/D53) — `Task.check`, red-check refusal at done,
  check-less-claim count in the digest header.
  done: §8 v4 tests green incl. the neutered-gate proof.
- v5 gets its row only when J1–J3 exist; until then it is a parked
  dependency note, not a row.

D-rows (texts only; claim numbers at land after re-reading the header —
0.35.0/D55 and 0.38.0/D57 were both lost to this):

- **D74 (placeholder) — executable acceptance on plan tasks.** Decision:
  `Task.check: Option<String>` (additive); `plan.update(done)` runs it via
  the goal check runner; red refuses with the evidence tail; the advisor
  digest counts check-less done-claims. Why: Factory's 36→90% result that
  self-authored completion criteria collapse; deterministic enforcement the
  model opted into, which D50 permits; the plan module's expand-only law
  supplies the freeze. Reversible-via: drop the field read and the header
  count; the field survives as unknown data (§19 rule 4).
- **D75 (placeholder, parked until J1) — offline search over evals.**
  Decision: GEPA-style reflective search over the evals workspace,
  optimizing Yi's own prompt assets as round-robin modules, human-promoted,
  deterministic μ. Why: acceptance requires execution (§0); the corpus
  supplies reflection material, the evals supply μ. Reversible-via:
  additive tooling; deleting it strands no data.

## 11. What we are NOT building

OMP-killed (each with its §1 evidence):

- **Scheduled/heartbeat mining, and any reflection loop on a counter
  trigger** — OMP's capture re-bills the whole conversation uncached per
  firing (sdk.ts:1145-1166); even the zero-token nudge is cut: a standing
  surface that earns nothing over the user noticing their own friction.
- **Stream-watching rules (TTSR-style interrupt/retry)** — mention/act
  confusion is structural to buffer-regex (export/ttsr.ts:362); Yi's
  act-scoped tool rules + the wall/permission engine already own this
  space.
- **LLM-judged creation, retention, retrieval, or deletion** — the
  division-of-labor law; OMP's model-gated skills with no human gate and no
  ledger are the counterexample (manage-skill.ts:40).
- **Uncapped per-item render text; whole-store prompt dumps** — the 16k
  single-skill and 5k memory-dump failure (managed-skills.ts:62-69,
  memories/index.ts:1400-1414); every Yi artifact carries a render cap and
  fires by trigger.
- **Write-only stores** — no artifact without an outcome ledger and a
  prune threshold; the report leads with deletions.

Self-review-killed (v1/v2, kept dead):

- Live prompt rewriting/paraphrase; per-prompt GEPA (invariant zero;
  negative-EV).
- Per-prompt retrieval into trusted blocks; augmenter extension; journal +
  undo; Template/Candidate/TaskSpec DTOs; recon compiler +
  spec-compiler.md; `flywheel.enabled` / `yi flywheel` / `models.miner`;
  Reviewer job enum; two-phase/Pareto/budget/xorshift before v5.
- Embeddings/vector store (linear scan over ≤ a few dozen strings is
  free); merge/crossover (no-op at this module count; hurt the smaller
  model in GEPA); multi-agent validator (the advisor is singular, §7.5);
  dsrs/gepars as dependencies; a second μ summarizer model;
  auto-persistence of anything (TRACE; the user landing every artifact is
  the feature).

## 12. Open questions

- **One-in-one-out ruling**: v0–v2 are skill text and process; v4 is the
  plan system's own acceptance law built out; v5 is where the §1.1 ruling
  genuinely bites. The plan assumes only v5 triggers it; the ruling is the
  user's.
- **Thresholds**: cluster ≥ 5 across ≥ 3 sessions; backtest ≥ 3 failure /
  ≤ 1 benign; prune at post-fire failure ≥ 0.6 over ≥ 5 fires or 30-session
  zero-fire streak; ≤ 12 rules; ≤ 400-byte bodies; ≤ 1,500-byte lessons —
  all proposed, set finally at P8 time with the corpus in view.
- **Lessons home**: AGENTS.md `## Lessons` (this repo: .ruler/ + apply) vs
  a dedicated project file loaded the same way. Default AGENTS.md; decide
  at first landing.
- **Rule-fire attribution**: body-text match first; the ~10-line
  `rule_fired` Record row only if real reports show ambiguity.
- **v5 shape**: revisit against whatever J1 actually builds; D75 stays a
  draft until then.
