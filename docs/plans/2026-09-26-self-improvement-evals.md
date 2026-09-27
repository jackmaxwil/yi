# Yi improves itself: four eval nouns, a cascade, and a gate that reads partial credit per task

```
status:  PROPOSAL, revision 2 (2026-09-26). Revision 1 audited what yi can be scored on
         and designed the closed loop: yi proposes a change, harbor scores it on the
         development tasks, a held-out validation gate accepts or rejects it, and every
         verdict is recorded. Revision 2 answers an adversarial review (§17). The review
         found a blocker in the gate's statistics and 17 major findings; the owner then
         took three more decisions (§13, round 4). Extends D140 (axes), D219 (graph
         refiner), D220 (levers) and D186 (the calibrated slice). Method: the yi-ideate
         skill.
tree:    main @ e0da990e (0.375.0), re-read 2026-09-27. Every ✓ was re-read there.
marks:   ✓ exists on main · ✚ new in this proposal · ⏸ deferred
issues:  #652 (P0) blocks #653-#657 and #681-#688; #689 follows #688; T11 is #76
```

## 0. Summary

Yi already has almost every part of a self-improvement loop. None of them has ever run
together, and three gaps keep the loop from starting:

- **The levers never reach a benchmark trial.** The harbor command carries neither
  `--eval` nor `YI_LEVERS` (`evals/adapters/yi_usage.py:133-139`). The 0.281.0
  changelog row says so under its limits.
- **The gates can only read suites that sit at the ceiling or at the floor.** The seven
  fixtures read 21/21 in rows 0036 through 0054; with `fan-out` added, row 0056 read 23/24
  for the base against 22/24 for the candidate, a one-trial wash. The six-task
  Terminal-Bench subset read 3/18 in
  rows 0018, 0021, 0023 and 0025, the same task passing each time. The twelve-task slice
  that D186 calibrated for partial credit near 0.5 (`evals/drivers/tbv4_slice.txt`) has
  never had a ledger row.
- **`levers.py` and `refine.py` have never paid for a run.** The 0.282.0 row says "Not
  run: any paid comparison", and `evals/fixtures/graph/rejected.jsonl` is empty.

This proposal adds no machinery that competes with those pieces. It names four nouns,
three of which exist. It merges the two gates into one that reads graded partial credit,
**one difference per task**. It wires the levers into harbor, and it orders the
evaluators cheapest-first.

| noun | what it is | status |
|---|---|---|
| **Candidate** | a base sha plus a patch and/or a levers file and a config (routing, skills, effort) | ✓ fingerprint (`yi_usage.config_fingerprint`, :281); ✚ one `candidate.json`; identity is the hash of the patch plus the levers object, never the rationale |
| **Runner** | `<runner> <candidate.json> <task>...`, which prints one trial row per trial | ✓ the protocol, twice (`levers.py:14`, `refine.py:13`); ✚ harbor through the existing sweep driver |
| **Trial row** | `task, reward, partialScore, input, cacheRead, output, costUsd, wallSec, censored` | ✓ `axes.py:79-102`, `:145-159` (all but `censored`, which is ✚); ✚ committed store `evals/trials/<run-id>.jsonl` |
| **Gate** | a paired verdict of a candidate against a fresh base, over one task group, one difference per task | ✓ `levers.interval` (:179-196), floors (:116-133); ✚ task-level pairing, graded metric, two roads |

A **verdict** is not a fifth store. It is the gate's output, written as the first row of
the run's trial file and as a column of that run's `docs/eval-ledger.md` row. The
rejection filter scans the trial files.

The **cascade** (cheapest evaluator first, stop at the first reject) and the
**proposer** (a `yi ask` session that writes a patch) are recipes: compositions of the
nouns and the binary, with no state of their own.

## 1. The problem, with evidence

**The gates read nothing that moves.** Rows 0018, 0021, 0023 and 0025 read 3/18 each:
`html-js-filter` passes 3/3, and the other five tasks pass 0/3. Across those same rows
the verifier tallies moved. Cargo went from 24, 19, 21 of 27 to 21, 25, 22. The
heat-pump partials went from 0.65, 0.85, 0.70 to 0.80, 0.70, 0.70. A binary gate threw
that signal away:

- `levers.measure` counts a task as solved when `reward > 0` (`levers.py:106`).
- `refine.score` counts passes and tokens per solved task (`refine.py:148-153`). When
  nothing is solved, that metric falls through to "guidance bytes unpaid"
  (`refine.py:161-162`).

**The levers cannot reach the benchmark.** `Levers::init` reads `YI_LEVERS` only under
`--eval` (`crates/runtime/src/levers.rs:145-155`). Two runners pass `--eval`: `run.py`
(:147) and `surface.py` (:130-133). The harbor command does not
(`evals/adapters/yi_usage.py:133-139`; `yi_harbor/agent.py:136-151`).

**Per-task history is lost.** Row 0055, the calibration sweep, found 19 of 38 tasks at
partial credit 0.2 to 0.8. The harbor job's directory was never kept, so those per-task
rows exist nowhere now. The drivers README names the same gap for `run.py`, which prints
per-task rows only to stdout (`evals/drivers/README.md:99-104`).

**Activation is not value.** F0e, row 0056, spent $15.06 and found "zero undirected use
of the new mechanisms". The candidate's directed cells cost $1.05 against the base's
$0.93 and scored no better.

**The model's own noise.** Every scored row uses glm-5.3-flash. Its reasoning spirals
arrive in upstream-side episodes (observed 2026-09-08: the endpoint, not a prompt change,
explained them). Row
0055 spread its turns over nine upstreams. The provider API takes no seed: `openai.rs`
sends `temperature` when one is given (:344-345) and never a seed. Pairing and pinning
the upstream are the only variance controls available.

**Where the money goes.** Row 0055's tokens are 25,711,091 uncached input, 99,237,504
cache reads and 2,133,204 output: a warm hit rate of 79.4%. At catalog prices
(`crates/ai/data/openrouter.json`: 0.075 input, 0.015 cache read, 0.25 output per
million) that is $1.93 + $1.49 + $0.53 = $3.95, so **uncached input is 49%**. The billed
total was $7.03 known, plus one unpriced turn. The cached prefix is 47,720 bytes (system
27,892 plus tools 19,828, `scripts/guardrails/baselines/request_budget.json`), about
11.9k tokens. Over 0055's 2,042 turns at the cache-read price, that is $0.37, **9.3% of
catalog**. Prompt bytes are a real but secondary lever. Cache hits and tool-result bytes
are the primary ones.

**Some signals the gate would read are wrong today.**
- `extract.py:419` counts `evidence_shape_refused` on the string "done needs evidence
  shaped". The live refusal reads "done needs evidence:"
  (`crates/runtime/src/todo/mod.rs:132`), so the signal reads 0 on the current binary
  while the refusal still fires.
- `length_forced` reads `details.forced` (`extract.py:326`), which no live message sets.
  The redrive details are `rung`, `cut` and `reasoningChars`
  (`crates/loop/src/run.rs:399-401`).
- A truncated tool call increments `length_stops` without writing a redrive
  (`run.rs:888-890`).

**Compaction never fires on the benchmark model.** glm-5.3-flash has a 1,048,576-token
window. Every tbv4, ARC and journey row reports 0 compactions, including peaks of
277,242 (0055) and 418,485 (0010). The trigger's scope matters on smaller windows:
- It charges only the growth past the first request's full input: the prefill latches
  at `compaction.rs:296-303`, and the subtraction is at `account.rs:151`.
- It fires when that growth exceeds `window − 16,384` (`compaction.rs:333-336`,
  `policy.rs:14-24`).

That scope is deliberate: `Scope::BodyAfterPrefix` is the default because the cached
prefix bills near a tenth of full price (design §4.4, `account.rs:139-140`). Its cost is
that the real total at trigger time is past the window whenever the first request exceeds
16,384 tokens: invisible at 1M tokens, and it matters on any 128k model. Charging the prefix
there would change a settled decision, so it is a proposal needing its own D-row (§14).

## 2. Laws, in the owner's words

From the request that opened this session:

> "The loop must stay deterministic: no scheduled LLM reflection and no auto-written memories."

> "Prefer live harbor runs over mocks, and never commit terminal-bench task files."

The budget, from round 4:

> "you can make the budget a soft and hard budget so we can comfoterbly fit 2 runs"

Standing laws carried forward:

- "the only part I have seen work at scale real world is data driven optimizations,
  driven by deterministic signals. We can't leave too much up to the LLM." (the owner's
  standing rule). On 2026-09-26 the owner overruled its trigger half for the classifier
  only (#587). No scheduled reflection and bounded injection still stand.
- Prompt-flywheel law 2: "The LLM writes gated prose and generates candidates and
  proposals; it never decides when anything fires, persists, ships, or dies."
- Budget discipline: "Timeouts are never retried." and "Defaults are what is
  benchmarked. A high-effort variant is a separate ledgered row"
  (`evals/README.md:487-491`).
- The value tolerance: "aggregate fully accounted cost and median task wall within **10
  percent** of the baseline … (the owner's number, 2026-09-13)"
  (`docs/archive/plans/2026-09-12-yi-operating-system.md` §10.3).

## 3. Deliverable 1 — the feature map

**Covered** means that some eval isolates the feature's effect on an outcome or on cost,
and that a gate or a ledger column reports it. **Partial** means the feature is
exercised by deterministic tests, or its signal is counted, but no eval measures its
benchmark effect. **Uncovered** means nothing measures it.

### 3.1 Prompt and context assembly

| feature | where | purpose | coverage |
|---|---|---|---|
| System prompt assembly | `crates/runtime/src/ext/assemble.rs:121`, `ext/install.rs:33` | slots by rank, three cache blocks joined by `\u{1d}` | partial: `request_budget.rs`, `prompt_drift.rs`, byte lock |
| identity.md, doctrine.md (5,628 + 21,279 bytes) | `crates/runtime/src/lib.rs:89-90`, `:94-95` | who yi is, its method | partial: journeys `ab.py` (one row, 0014); never A/B gated |
| Permission-mode fragment | `crates/permission/src/decide.rs:20-31` | tells the model the policy | uncovered: benchmarks run `--yolo` |
| Skills catalog | `crates/runtime/src/skills.rs:60-104` | `<skills>` block, 8–32 KiB | partial: lever L1 in rows 0006/0007 (−45% on trivial tasks); benchmark trials install no skills (`justfile:343-346` is never run by an adapter) |
| Project instructions | `crates/runtime/src/ext/project.rs:164` | AGENTS.md/CLAUDE.md, trust-pinned | uncovered |
| Language pack (har-core) | `ext/install.rs:13,24` | attached on Cargo.toml or the first `.rs` write | uncovered |
| grid fragment | `crates/runtime/src/ext/grid.rs:54` | only when `grid` is on PATH | uncovered (absent in task images) |
| Environment block | `crates/runtime/src/environment.rs:177` | a trailing per-request message | partial: `request_budget.rs:661` (the prefix does not move) |
| Memory block | `crates/runtime/src/memory/store.rs:12-13` (200 lines, 25,000 bytes) | recalled notes | uncovered in benchmarks (fresh HOME) |
| Route prefilter | `crates/runtime/src/ext/orchestrate.rs:86`, consts `:134-137` | keyword score; Complex attaches orchestrate.md | partial: `orient_census.py`; rows 0002/0003 found it degenerate |
| Graph `next:` lines | `crates/runtime/src/affordance.rs:11,26` | ≤2 hints per tool result | partial: `refine.py` exists, never paid |
| Compaction trigger | `crates/context/src/policy.rs:20`, `crates/runtime/src/compaction.rs:326-337` | summarize past the prefill | uncovered on flash (0 compactions in every row); cassette `compaction-constraint-survival` covers correctness |
| Retention floor | `crates/context/src/floor.rs:5` | user messages verbatim, 64k | partial: unit tests |
| Per-source byte budgets | `crates/context/src/budget.rs:17-20` | instructions, skills, ledger, memory caps | partial |
| Internal-context wrapper | `crates/context/src/wrapper.rs:49` | dropped at compaction; nudges are not in its list | partial |

### 3.2 Tools (10 model-visible, `scripts/guardrails/baselines/tool_surface.json`)

| feature | where | purpose | coverage |
|---|---|---|---|
| read | `crates/tools/src/hashline/tool.rs:54`, caps `:17-24` | hashline read, 2,000 lines | covered: `surface.py` refusal rate, `mu.jsonl` per-tool counts |
| edit | `hashline/prompt.md` (133 lines), `tool.rs:819`, no-op guard `:27` | anchored edits | covered: `surface.py` (17 scenarios) |
| write | `crates/tools/src/builtins.rs:37` | whole-file write | covered: `surface.py` |
| bash + reducer | `builtins.rs:506`, `reduce.rs:11-20`, `process.rs:11` (30,000 B) | shell; over 8,192 B keeps head 80, tail 40, grep 60 lines | partial: `reduced_results`, `pointer_never_read` counted, never gated |
| grep | `crates/tools/src/grep.rs:721` | paged search | partial: `broad_search_refused` |
| get_context | `crates/tools/src/orient.rs:57` | orientation packet | partial: `orient_census.py` P13, not ready |
| ipython / kernel | `crates/tools/src/ipython.rs:46`, `crates/kernel/src/lib.rs:24` (65,536 chars) | Python cells, `rlm` | partial: `kernel_cells`, `kernel_dead` in rows 0024/0025 |
| todo + coupling | `crates/runtime/src/todo/tool.rs:13`, `todo/coupling.rs:22-28` | the list and its nudges | covered: fixtures `five-items`, `seen-red`; persistence axis; levers tunable |
| plan + coupling | `crates/runtime/src/plan/tool.rs:767`, `plan/loop_coupling.rs:17-25` | plans, delegation | partial: directed surface runs 0057-0059; 0056 found zero undirected use |
| ask_user | `crates/runtime/src/auto_review.rs:176-180` | block on the user | covered: fixture `block-on-user` |
| subagents / family | `crates/runtime/src/subagent.rs:27,29`, `family.rs:9` | fan-out | partial: fixture `fan-out`, directed surface runs |
| judge / jury | `crates/runtime/src/plan/judge.rs:19,22` | contract verdicts | partial: `juror_*` signals |

### 3.3 Loop control

| feature | where | purpose | coverage |
|---|---|---|---|
| Reasoning cut | `crates/loop/src/reasoning.rs:7` (48,000 chars), `run.rs:380-420` | cut a spiral, quote it back | partial: `spiral_cut` counted (24 in 0025); not tunable |
| Length / cut stop | `crates/loop/src/run.rs:383` (3), `:386` (6); truncated calls `:888-890` | end the run after repeated stops | partial; `length_forced` is a dead signal (§1) |
| Repeat breaker | `run.rs:509-513` | steer at 3, stop at 6 in a 6-turn window | covered: rows 0020 (D152), 0037 (D179) |
| Stream retry | `run.rs:467-473` | rerun an empty error turn once | partial: `stream_retry` |
| Deadline + last word | `crates/runtime/src/session/deadline.rs:6-9`, `crates/loop/src/run.rs:848` | stop before harbor kills the trial | covered: row 0035 (D177), E12 |
| No max-turns cap | `run.rs:756` | — | n/a: the deadline, the breakers and the watcher bound a trial |
| Tool-name repair, leaked-call recovery, JSON salvage | `crates/loop/src/repair.rs:3`, `crates/ai/src/leak.rs:16`, `crates/types/src/json_salvage.rs:17` | recover malformed calls | partial: unit tests |

### 3.4 Provider, routing, caching

| feature | where | purpose | coverage |
|---|---|---|---|
| OpenRouter default routing | `crates/ai/src/openai.rs:33-36`; `EVAL_ROUTING` (`yi_usage.py:45-58`) | throughput/latency preference | covered: routing A/B rows 0031-0033, upstreams column (D174) |
| HTTP retry | `crates/ai/src/retry.rs:11-16` (3 attempts, 300 s total), backoff cap 8 s at `:49` | transient errors | partial |
| Settling cut turns | `crates/ai/src/settle.rs:37` | price turns cut mid-stream | partial: an unpriced turn still stopped drivers (rows 0025, 0056) |
| Anthropic cache breakpoints | `crates/ai/src/anthropic.rs:52,234-249` | 3 system blocks plus the newest message | covered: live lane `cache-warm`, `request_budget.rs:335-384` |
| OpenAI cache key | `openai.rs:325-328`, only when `base_url` is api.openai.com | `prompt_cache_key` = session | covered for OpenAI; **not on the OpenRouter path flash uses** |
| Upstream cache on flash | — | whatever the upstream does | partial: `cache_probe.sh` (row 0013, a warm miss on glm); hit rate in the experience column |
| Effort default (Medium) | `crates/types/src/model.rs:12`; mapping `openai.rs:363-372` | reasoning effort | uncovered: no row varies it |
| Output ceiling | `crates/runtime/src/provider.rs:234` (32,768) | max output per request | uncovered |
| Cost | `crates/ai/src/catalog.rs:112`; E14 provider cost | spend | covered: E9/E14 cross-checks |

### 3.5 Permission and sandbox

| feature | where | purpose | coverage |
|---|---|---|---|
| Decision ladder | `crates/permission/src/decide.rs:125-232` | catastrophic, rules, mode | partial: `crates/permission/tests/*`; benchmarks are Yolo (`decide.rs:218`) |
| Command classifier | `crates/permission/src/safety.rs:356,402` | safe / destructive / contain | partial |
| Seatbelt | `crates/tools/src/sandbox.rs:7,159` | macOS containment | partial: `surface.py` on the Mac; latency unmeasured; Linux has no sandbox, so Contain becomes Ask, which headless becomes Deny (`crates/runtime/src/permission.rs:335-362`, `gate.rs:77-79`) |
| Auto-review | switch at `crates/cli/src/main.rs:329-331` (naming `models.autoReview` turns it on); timeout `auto_review.rs:15` | LLM permission reviewer | uncovered (off unless configured) |
| Classifier role (#587, Laya) | not in the tree | skill triggers, auto-allow | uncovered (proposal only) |

### 3.6 Session, CLI, measurement

| feature | where | purpose | coverage |
|---|---|---|---|
| Session JSONL v4 | `crates/types/src/wire.rs:15`, `crates/session/src/jsonl.rs:62` | the one ledger | covered: conformance tests; every eval reads it |
| Telemetry sidecar | `crates/runtime/src/telemetry.rs:71` | ttft, tokens, cost per span | covered: live lane, `axes.py` experience ttft |
| Levers | `crates/runtime/src/levers.rs:66-111` (44 listed, 25 tunable) | eval-only overrides | partial: `run.py` and `surface.py` only |
| `yi ask --json --eval --deadline` | `crates/cli/src/ask.rs:15`, `main.rs:127-128` | the harness entry | covered: E1, `selftest.py` |
| **Trial watcher** | `evals/drivers/watch.py:32-33,121` (a trial past $1 or 180 turns is stopped) | censors runaways | partial: shapes every sweep number (0055's $0.13 median), and the gate ignores it today |
| Kernel bootstrap, prewarm | `crates/kernel/src/bootstrap.rs:829`, `crates/runtime/src/wiring.rs:94-96` | venv, boot | partial: the adapter's `yi doctor --fix` |
| Session-mining extractor | `skills/yi/session-mining/extract.py:260-278` | μ, issues, signals | covered, with the two stale signals of §1 |
| console, tui, orb, acp, mcp-cli, oauth | `crates/{console,tui,orb,acp,mcp-cli,oauth}` | surfaces | no benchmark effect; drive, PTY and protocol tests |

**Reading.** The tool surface and the todo loop are covered, because `surface.py` and
the fixtures were built for them. Everything that moves Terminal-Bench cost or partial
credit is partial or uncovered: upstream caching, tool-result bytes, the loop guards,
effort, the watcher's censoring. And the harness cannot carry a change to any of them
into a trial.

## 4. Deliverable 2 — optimization targets

Axes are D140's five (`docs/archive/plans/2026-09-06-tbv4-evals/axes.md`): A Pass, B
Persistence, C Rigor, D Experience, E Economy. SNR is the expected effect over the noise
of the task-level paired measurement.

Costs use row 0055's median priced trial, $0.13. That median is itself censored: the
watcher stops a trial past $1 or 180 turns, so it holds only while the runner keeps the
same caps (§6.3).

| # | target | lever | metric → axis | eval | SNR | cost/run |
|---|---|---|---|---|---|---|
| T1 | **Upstream pin and cache hit** | `EVAL_ROUTING` ✓ (config, rides the fingerprint) | warm hit rate, uncached input, `costUsd` → E | N4 probe, then the cascade (economy road) | high: pinning at a 0.9 hit rate cuts catalog cost ~20% ($3.95 → $3.16) with output and tool-result bytes held fixed; 95% (~30%) is a hypothetical ceiling | <$0.05 probe; $4.68 dev |
| T2 | **Loop guards against spirals** | `loop.cut_stop_at`, `loop.length_stop_at` ✓ listed, ✚ tunable through `LoopConfig`; `loop.reasoning_cap` too, without a census | graded, `spiral_cut`, wall → A, E | N5 census (cut count only), then the cascade | medium: 24 cuts in 0025; 0056 stopped on length-stops; episodic, so it needs the pin | $0 census; $4.68 dev |
| T3 | **Tool descriptions and refusal texts** | patches to `hashline/prompt.md`, `builtins.rs` descriptions | refusal rate per tool, turns → E, C | N6 `surface.py`, then the cascade | high on refusals (0.283.0: 30 false edit refusals from one corpus); unknown on partials | $0.25-0.60; $4.68 |
| T4 | **Done/evidence loop** | `plan.done_refusal_cap`, `todo.intercept_cap` ✓ tunable; `todo.nudge_work`, `todo.empty_stop_cap` ✓ but no census | turns, graded → C, B, E | **recount first** (the signal is stale, §1), then N5 on intercept records, then the cascade | unknown until recounted: "19 in 0025" was counted against the refusal text of 2026-09-09 | $0 recount; $4.68 |
| T5 | **Tool-result bytes** | `tools.reduce_floor` (the description names the number, so a patch), `OUTPUT_CAP` ✚ (`process.rs:11`), read caps ✚ | uncached input per turn, `pointer_never_read` → E, guarded on A | the cascade (economy road); no offline replay, since sessions hold only post-reduction bytes | medium: uncached input is 49% of catalog | $4.68 |
| T6 | **Compaction threshold** | ✚ `context.compact_at` (`Settings` has no config key, `policy.rs:14-15`) | peak context, cache reads, graded → E | offline predictor over committed rows (peak × turns), then the cascade | low-medium: never fires on flash; peaks of 277k-418k are paid on every later turn | $0; $4.68 |
| T7 | **Reasoning effort** | config `thinking` ✓ (`crates/types/src/config.rs:28`) | graded vs cost → A, E | cascade, a separately ledgered row | medium; cost likely past +10%, so road 1 rejects it by rule | $4.68+ |
| T8 | **Doctrine and identity text** | patches to `prompts/*.md` | graded, rigor, experience → A, C, D | journeys `ab.py` ✓ ($0.17), then the cascade | low: diffuse; the cached prefix is 9.3% of catalog | $0.17; $4.68 |
| T9 | **Skills in the trial HOME** | ✚ `EVAL_SKILLS` adapter switch | graded, rigor → A, C | cascade | low, unknown sign | $4.68 |
| T10 | **Graph `next:` lines** | graph.json patches (`refine.py` structural checks ✓) | graded → A | cascade | low (≤2 lines per result) | $4.68 |
| T11 | **Route prefilter** | `route.*` ✓ tunable | label flips | `orient_census.py` ✓ | expected zero flips (0003: degenerate) | $0 |
| T12 | **Plan width, family caps** | `plan.width_max`, `family.*` ✓ | activation | census | zero undirected activation in 0056 | $0 |
| T13 | Permission and sandbox cost | none on the benchmark path | refusal rate (Mac), latency | `surface.py` ✓; latency ⏸ | none on Terminal-Bench: trials are Yolo on Linux | — |
| T14 | Classifier (#587) | not in the tree | trigger precision | ⏸ | — | — |
| T15 | Latency (ttft) | routing | ttft p50 → D | live lane ratchet ✓ | not a score lever | — |

Retries and unpriced turns are not a score lever; they are a precondition. An unpriced
turn stopped the driver in rows 0025 and 0056, and the gate's `unmeasured` rule would
otherwise reject candidates for a provider failure. Settling them is in §8 stage 0.

## 5. Deliverable 3 — the self-improvement loop

### 5.1 Precedents

- **Karpathy's autoresearch** (<https://github.com/karpathy/autoresearch>): an agent
  edits one file, trains for a fixed five minutes, and keeps the change if `val_bpb`
  improved.
  `program.md` maps to the proposer's brief, `train.py` to the lever surface, and the
  fixed budget to the soft/hard caps. **The one deliberate difference:** autoresearch
  trusts a single validation number. Agent evals are small-N and clustered by task, so
  here the dev group only screens. A task-level paired interval on a 12-task validation
  group decides, corrected for how many times that group has been looked at.
- **Successive halving / Hyperband**: spend a little on every candidate, and a lot only
  on the survivors.
- **Reusable holdout** (Dwork et al., 2015): every validation access spends from a fixed
  budget, and the threshold tightens with the budget (Bonferroni over the accesses, the
  simplest instance). When the budget is spent, the final group runs, retires into
  validation, and a new final group is drawn (OS plan §10.4).
- **A/A testing**: measure the noise floor with two identical arms before trusting any
  A/B.
- **Candidates are commits**: prompt-flywheel §8 ("git is store, review is gate, ratchet
  is regression net").

### 5.2 What exists and what is missing

| part | status | where |
|---|---|---|
| Order-statistic interval (the inverted sign test) | ✓ | `levers.py:179-196` |
| Floors per class; a cheaper candidate below a floor is rejected by class | ✓ | `levers.py:116-133`, `floors.json` |
| Structural checks for graph edits | ✓ | `refine.py:55-115` |
| A proposal naming a held-out task is refused | ✓ | `refine.py:170-194`, `levers.fit_rows` :153 |
| Rejection filter by hash, base and config | ✓ (graph only) | `refine.py:197-244` |
| Config fingerprint with the levers hash | ✓ | `yi_usage.py:70-80`, `run.py:340,441` |
| Soft/hard spend caps, per-trial watcher | ✓ | `tb21_cost.py --soft/--hard`, `watch.py` |
| Trial rows with partial credit | ✓ | `axes.py:79-100` |
| **E16: levers inside a harbor trial** | ✚ | adapter uploads the file and passes `--eval` |
| **Harbor as a runner** | ✚ | `tbv4_sweep.sh` + `watch.py` take a candidate binary and a task list; no new driver |
| **Task-level pairing, graded metric, two roads** | ✚ | `levers.per_task`, `judge` |
| **Trial store with verdict rows** | ✚ | `evals/trials/<run-id>.jsonl`; sessions under `<runs>/<run-id>/`, where `<runs>` is a per-host runs-directory setting outside the repo, as `TBV4_RUNS_DIR` is today; `rejected.jsonl` folds in |
| **Split and floors for benchmark tasks** | ✚ | `split.json` gains `tbv4/<id>` entries |
| **Loop guards tunable** | ✚ | runtime copies `levers::get()` into `LoopConfig`; `run.rs` and `ReasoningBudget` read those fields; yi-loop never calls `levers::get()` (it may depend only on yi-types, `scripts/guardrails/boundaries.toml`) |
| **Census in the extractor** | ✚ | `extract.py` gains the flip counts; its two stale signals are fixed |
| **Proposer container and round recipe** | ✚ | `evals/improve/brief.md`, `just improve` |
| Semantic search over past sessions | ⏸ owned by the memory agent | §5.5 |

### 5.3 One round, end to end

The owner types `just improve`. Nothing else starts a round.

1. **Base.** The musl build of `origin/main` at a recorded sha. Its fingerprint names the
   binary, the model, the pinned `EVAL_ROUTING` and the suite digest (E13). **Every
   candidate gets its own fresh base arm,** interleaved with it in one job. No base
   sample is reused across candidates.

2. **Propose.** The proposer is `yi ask` running with `evals/improve/brief.md`, inside a
   container. The container mounts exactly two things:
   - a `git archive` snapshot of the base sha, with no `.git`, and with
     `docs/eval-ledger.md` and `evals/trials/` removed;
   - a corpus directory holding only dev-task trial rows and dev-task session JSONL,
     including the dev sessions of earlier rejected candidates.

   It returns at most three patches, each with a `candidate.json` of
   `{levers, config, target, rationale}`. The round applies each patch on the host.

3. **S0, $0.** Deterministic refusals:
   - The patch touches only the lever surface: `crates/runtime/src/prompts/`,
     tool-description files, and the levers and config objects.
   - If it moves a locked baseline (`tool_surface.json`, `request_budget.json`), the
     ratchet is its **own commit ahead of** the code commit. `check_commit_style.py:133-154`
     refuses a baseline edit in a commit that touches code; the order is repo law. The
     candidate hash covers both.
   - `just check` is green.
   - The candidate's hash (patch plus levers object) is not in any verdict row on the
     same base.
   - An integer lever flips at least one census decision (N5).
   - A text patch whose predicted prefix cost (§6.7) rises by more than 10% is refused.
     The owner may waive that on the PR; the rationale never can.

4. **S1, fixtures, k=3, paired.** About $0.61 (row 0056: $0.299 and $0.308 per arm).
   The check is the `fixtures` class floor, with a tolerance taken from N1.

5. **S2, one dev task, k=1.** About $0.13. Mechanics only: the run is measurable, has
   no `refusal:config`, and does not crash. It is not scored.

6. **S3, dev screen, 6 tasks × k=3, paired.** About $4.68. Screen rule: the median
   task-level graded difference is ≥ 0, no binary pass is lost, and no floor is broken.
   A screen cannot accept anything; it only decides whether validation is worth buying.

7. **S4, validation, 12 tasks × k=2, paired.** About $6.24. This spends one validation
   access. It is the gate of §6.3. On a pass, the round opens a promotion PR with the
   patch, the ledger row, the trial rows and the verdict.

8. **The owner merges or closes it.** The final group runs once per release, or when
   the 4-access budget is spent.

**Budget.** A candidate that reaches validation costs about $11.66 ($0.61 + $0.13 +
$4.68 + $6.24).

- Each stage has a soft cap of $8 and a hard cap of $10. The owner's "$10/run" is per
  stage (round 4).
- **The week has a soft cap of $25 and a hard cap of $30**, approved by the owner on
  2026-09-27. A candidate starts only when
  the week's spend plus its predicted cost fits under $25. The hard cap stops the
  stream mid-stage. These are the existing `tb21_cost.py` semantics, applied to the
  week's summed ledger rows.
- Two full candidates cost $23.32, under the soft cap: "so we can comfoterbly fit 2
  runs".

**Wall time.** Row 0055 ran 38 trials on 3 slots in 7 h 45 min, about 37 minutes per
slot-trial. At 6 slots on the Buildhost, that makes S3 (36 trials) about 3.7 h and S4 (48
trials) about 5 h.

### 5.4 Why it stays deterministic

- No schedule. A round starts only when the owner types a command.
- The LLM writes patches, and nothing else. It decides no refusal, no stop, no
  deduplication, no acceptance and no spend; those are counters, intervals and caps.
  Its rationale is never an input to any of them.
- A promotion ships only through a PR the owner merges.
- Verdict rows are machine-written data, and no model reads them in a prompt. They act
  only as a filter.

### 5.5 The proposer's corpus and the leakage rule

The owner's decision (verbatim in §13): the proposer "can read past session jsonl", and
semantic search over past sessions is wanted, with the full memory system built by
another agent.

- **Leakage is a mount rule, not a filter on an index.** The proposer cannot open
  anything the container does not mount. The snapshot has no git history, no eval
  ledger and no trial store, because PR history and the ledger both carry validation
  rows (0055's notes name tasks and partials). The corpus holds only rows and sessions
  whose task id is in `split.json`'s development group, selected by the token rule of
  `refine.names`. A test plants a validation task id in the host's trial store and
  proves it is absent from the container.
- **Interface for search, when it lands:**
  `search(query, k) → [(session_path, entry_seq, snippet)]`, over the mounted corpus
  only.
- **Tension to resolve.** The 2026-09-10 memory plan rules embeddings out: "No
  embeddings, no LLM judge" (`docs/plans/2026-09-10-memory.md:983`; `:888`, "until grep
  fails"). The memory agent should record whether the owner's new ask supersedes that
  plan or is scoped to the proposer.

## 6. Deliverable 4 — new evals

**The graded score, everywhere:** `partialScore` as `axes.py` fills it (a trace's
`partial_score` or `diagnostic_score`, else `trace_summary.json`'s passed/total), else
`reward`. **Never the ctrf tally.** That tally can be a wrapper test that always passes:
`selftest.py:323-335` pins freight-dispatch-shift's one-test wrapper beside a 0.56 trace
score, and vba-userform-port's four passing wrappers beside 7/28 traces.

### 6.1 N1: A/A calibration of the slice

- **Task source.** `evals/drivers/tbv4_slice.txt`, twelve tasks. The dataset is pinned
  by digest (E13); task files never leave harbor's cache.
- **Scorer.** Trial rows, graded as above.
- **Output (a measurement, not a pass/fail).** From the task-level differences between
  two identical arms:
  - each task's mean and spread;
  - the distribution of the 12 task-level differences, which sets δ (§6.3) and the
    class tolerances;
  - cost and turn spread.
- **No task is dropped on this sample.** A task that reads 0 or 1 in both arms stays in
  the split until a later k shows the same floor or ceiling.
- **Variance plan.** k=2 per arm, base binary in both arms, arms interleaved, task order
  seeded by the run id, the upstream pinned (after N4), and the watcher's caps on:
  48 trials.
- **Spend.** Soft cap $6.50, hard cap $8.
- **The prior it replaces.** Rows 0023 and 0025 give per-trial graded SDs of 0.081
  (cargo), 0.075 (heat-pump), about 0.03 (bun) and 0.264 (foodstuff). That spans two
  builds and four tasks, so it is an upper bound on trial noise and says nothing about
  the task-level spread the gate needs.

### 6.2 N2: drawing the validation extension and the final group

- **Task source.** The 36 tasks of `tbv4_sweep_tasks.txt` that are not in the slice.
  Exclude `wal-recovery-ordering`, `glycan-ms2-elucidation` and `ontology-kg-querying`
  (0.85-0.98 in 0055). Draw 16 by a seeded shuffle.
- **Threshold.** Keep the tasks graded in [0.2, 0.8] at k=1, up to 12. The first 6 in
  shuffle order extend validation; the next 6 are the final group. A shortfall draws
  the next 4.
- **Variance plan.** k=1: this is selection by band, not measurement. The selected tasks
  never enter the proposer's corpus.
- **Spend.** Soft cap $2.50, hard cap $3.

### 6.3 N3: the gate (S3 screen, S4 validation, final)

- **Groups.**
  - Dev: 6 slice tasks.
  - Validation: 6 slice tasks plus the 6 from N2, 12 in all.
  - Final: 6 from N2.
  - The slice is split between dev and validation by stratifying on the N1 mean
    (sorted, assigned alternately, ties broken by the sha256 of the id).
- **One difference per task.** For each task, the difference is the candidate's mean
  graded score over its k trials minus the base's. The interval is `levers.interval`
  over the task differences, never over (task, repetition) pairs, because repetitions of
  one task are one cluster (the 0.282.0 limits say so).
- **Validation is corrected for reuse.** With A = 4 accesses per final draw, each access
  asks for confidence 1 − 0.05/A = 0.9875. At 12 tasks, `levers.interval` then allows at
  most **one non-positive task** (reported confidence 0.9937; computed).
- **Rejected outright** (closed vocabulary):
  - `below_floor:<class>`, `task_not_run:<task>` and `class_not_run:<class>` (existing);
  - `pass_lost`: fewer binary passes than the base over the scored tasks;
  - `unmeasured`: a trial without a price after stage-0 settling.
- **Failures and censoring.** A one-sided provider failure, or a one-sided watcher stop,
  scores the failing arm's trial as a loss (graded 0). A stop is part of what the
  candidate did. A (task, repetition) is dropped only when both arms are unusable. More
  than 20% dropped makes the run `pairs_dropped`, which is inconclusive.
- **Road 1 (capability).** The graded interval's low bound is above 0, and the cost
  interval's high bound on per-task relative cost differences is at most +10%. The same
  holds for median wall. A point estimate inside +10% is not a pass.
- **Road 2 (economy).** An efficiency interval's high bound is below 0 (today's rule,
  `levers.py:233`, applied per task), and the graded interval's low bound is above −δ.
  δ comes from N1's task-level A/A distribution: its 90th percentile of |difference|,
  and always smaller than the smallest gain road 1 could accept. It is never copied
  from a t-test MDE.
- **Otherwise inconclusive.** A verdict row is written, and the same candidate hash may
  not re-run on the same base.
- **The dev screen** (S3) uses the same per-task differences with the looser rule in
  §5.3, step 6. **The final group** is a regression check, not a significance test:
  `pass_lost` or `below_floor` blocks the release, and the graded differences are
  reported.
- **Variance plan.** k=3 on dev, k=2 on validation and final. Arms interleaved by
  repetition (`levers.py:264-269`). A fresh base per candidate. The pinned upstream
  (`EVAL_ROUTING` `order`, `allow_fallbacks:false`), the suite digest and the binary
  sha. Task order seeded by the round id. No sampling seed exists.
- **Spend.** Soft cap $8 and hard cap $10 per stage; soft $25 and hard $30 per week.

### 6.4 N4: the upstream pin probe

- **Task source.** `cache_probe.sh`, extended with a sample count (✚; no such knob
  exists today). One routing object per upstream seen in 0055: Together, Relace,
  Parasail, Wafer, Z.AI. Three two-turn sessions each: 30 turns in total, cent-scale.
- **Scorer.** `cache_probe.py`: warm-turn hit rate, 429s and errors, cost per turn.
- **Rule.** Pin the cheapest upstream whose warm hit rate is ≥0.9 in all three samples
  and which returned no 429. If none qualifies, take the highest hit rate with
  fallbacks allowed, and the ledger row says so.
- **Spend.** Guard at $0.10.

### 6.5 N5: the activation census (offline, $0, inside `extract.py`)

- **Task source.** Dev-group session JSONL.
- **Scorer.** Only the predicates whose inputs are on the recorded entries:
  - `loop.cut_stop_at`: consecutive `length_redrive` entries with `cut: true`
    (`run.rs:399`);
  - `todo.intercept_cap`: `TodoInterceptRecord` `rung` and `cycle_total`
    (`crates/types/src/todo.rs:293-298`);
  - the route thresholds: route features, as `orient_census.py` reads them.
- **Not census-able until the session records the counter.** `loop.reasoning_cap`: a cut
  aborts the stream at the cap, so no longer reasoning exists to count.
  `loop.length_stop_at`: truncated calls count without a redrive (`run.rs:888-890`).
  `todo.nudge_work`: a nudge does not carry the work count. These go straight to the
  cascade, and S1 is their cheapest screen.
- **Threshold.** Zero flips refuses the candidate with `no_activation`.
- **Fixes that land with it.** Point `evidence_shape_refused` at the live string
  ("done needs evidence:", `todo/mod.rs:132`), and drop `length_forced` or record
  `forced` at the source.
- **Pinning.** `selftest.py` pins each census predicate against a Rust fixture.

### 6.6 N6: the surface pre-screen for tool-text patches

- **Task source.** `evals/fixtures/surface/scenarios.json` (17 scenarios).
- **Scorer.** `surface.json`: the refusal rate per tool.
- **Threshold.** No tool's refusal rate above the base's, and no scenario that was clean
  on the base now unclean.
- **Variance plan.** k=2 per scenario, interleaved with the base.
- **Spend.** `--cap-usd 1`.

### 6.7 Not new: the prefix-cost predictor

Predicted cost = Δ(system + tools bytes)/4 × Σ recorded dev turns × the cache-read
price. The bytes come from `request_budget.json`, and the turns from the trial rows.
This is a recipe inside S0: above +10%, the patch is refused unless the owner waives it
on the PR.

## 7. Deliverable 5 — ranked backlog

The expected gain is estimated from the evidence in §1 and §4. The cost is that of the
cheapest stage that can decide.

| rank | target | expected gain | deciding cost | first paid run (one task) |
|---|---|---|---|---|
| **1** | T1 upstream pin and cache (#653) | ~20% catalog cost at a 0.9 hit rate (computed), and lower variance for every later row | <$0.05 + a candidate | N4 probe, then one slice dev task, k=1, pinned (~$0.13) |
| **2** | T2 loop guards (#654) | recovers trials lost to spirals (photonic 0/3 in every subset row; 0056 stopped) | $0 census + a candidate | the dev task with the most `spiral_cut` in N1, k=1, at the census value ($0.13-0.78) |
| **3** | T3 tool descriptions and refusal texts (#655) | fewer wasted turns (0.283.0 precedent) | $0.25-0.60 | one surface scenario, k=1 (~$0.03), then one dev task, k=1 |
| **4** | T4 done/evidence loop (#656) | unknown until the signal is recounted | $0 recount | first a $0 recount on N1's sessions; then one dev task, k=1, with `plan.done_refusal_cap` (~$0.13) |
| **5** | T5 tool-result bytes (#657) | a share of the 49% uncached-input cost | a candidate | one dev task, k=1, with the reducer patch (~$0.13) |
| 6 | T6 compaction threshold (#682) | cost on long trials | $0 + a candidate | — |
| 7 | T7 effort (#683) | graded, likely outside +10% | a candidate | — |
| 8 | T8 doctrine text (#684) | diffuse | $0.17 + a candidate | — |
| 9 | T9 skills in the trial HOME (#685) | unknown sign | a candidate | — |
| 10 | T10 graph lines (#686) | small | a candidate | — |
| 11-12 | T11 route (#76), T12 plan/family (#687) | expected zero | $0 | — |
| ⏸ | T13-T15 | no benchmark path | — | — |

P0 (#652) is the prerequisite for every row. Its first paid run is one slice task, k=1,
through the Buildhost (~$0.13), before N1.

## 8. Build order

Each stage is a demonstration and a gate. The worst pain comes first: nothing measurable
reaches a benchmark trial today.

| stage | builds | demo | gate | paid |
|---|---|---|---|---|
| 0 (#652) | E16. Harbor runner via `tbv4_sweep.sh` + `watch.py` (candidate binary, task list, `censored` rows). Trial store with verdict rows. Task-level gate, graded metric (no ctrf), two roads, reason codes, Bonferroni access count. Settle unpriced turns so the driver never stops on one. `extract.py`'s two stale signals. | a faux-tier harbor run whose fingerprint carries `+levers<hash>`; one row in `evals/trials/` | `selftest.py` pins E16, the graded metric, the per-task pairing, both roads, the censor rule and every reason code | $0.13 (one task) |
| 1 (#652, #653) | N4 pin probe; N1 A/A; fill the dev half of `split.json` and `floors.json`, and set δ | ledger rows; the task-level difference table | tolerances and δ written from data | ~$6.50 |
| 2 (#681, #653) | N2 draw; the T1 verdict | the first real verdict row | N4's rule; the gate | ~$14 |
| 3 (#654) | `LoopConfig` fields read by `run.rs`/`ReasoningBudget`; N5 census; T2 | the census flip count; the T2 verdict | `crates/loop` tests prove defaults unchanged; `levers::the_manifest_matches`; `boundaries.toml` unchanged | ~$12 |
| 4 (#688) | Proposer container, `brief.md`, `just improve`; `rejected.jsonl` folds into the trial store | one owner-started round | the planted-validation-id mount test; S0 refusals pinned | ≤$25 week |
| 5 (#689) | Semantic session search (memory agent) mounted into the proposer | — | the same mount test | — |

## 9. What this deletes or merges

- `refine.score` and `refine.worse` (`refine.py:148-167`) merge into the gate. The graph
  refiner keeps its structural checks and becomes a patch source.
- `evals/fixtures/graph/rejected.jsonl` folds into the verdict rows of the trial store.
  No separate verdict ledger is built; the eval ledger's row carries the verdict column.
- No new harbor driver. `tbv4_sweep.sh` and `watch.py` take the candidate.
- No separate census script. The census lives in `extract.py`, which already counts
  these signals.
- `evals/drivers/tbv4_baseline.sh`'s six-task subset retires: 3/18 in four rows, and no
  signal a gate can use.
- `split.json`'s fixture-only groups are replaced; the fixtures stay as the S1 class.

## 10. Exists / new inventory

| piece | status | file |
|---|---|---|
| interval, floors, survivors | ✓ | `evals/levers.py` |
| structural graph checks, held-out name refusal | ✓ | `evals/graph/refine.py` |
| trial rows with partials | ✓ | `evals/axes.py` |
| soft/hard caps, watcher | ✓ | `evals/drivers/tb21_cost.py`, `watch.py` |
| cache probe | ✓ | `evals/drivers/cache_probe.sh` (✚ sample count) |
| surface loop | ✓ | `evals/surface.py` |
| E16 | ✚ | `evals/adapters/yi_usage.py`, `yi_harbor/agent.py` |
| task-level gate, two roads, reason codes | ✚ | `evals/levers.py` |
| trial store, verdict rows | ✚ | `evals/trials/` |
| harbor runner | ✚ (extension) | `evals/drivers/tbv4_sweep.sh`, `watch.py` |
| census, stale-signal fixes | ✚ (extension) | `skills/yi/session-mining/extract.py` |
| loop levers | ✚ | `crates/loop/src/config.rs`, `run.rs`, `reasoning.rs`; `crates/runtime/src/levers.rs:103-107` |
| proposer container, brief, round recipe | ✚ | `evals/improve/`, `Justfile` |

## 11. Worked example: T1 from probe to verdict

1. N4 probes five upstreams, three sessions each, for about a cent. Say Parasail reads
   warm hits of 0.96, 0.95 and 0.97 with no 429, and is the cheapest that qualifies.
2. The candidate is `{patch: none, levers: {}, config: {routing: {"order":["parasail"],
   "allow_fallbacks":false}}}`. S0 costs $0: no patch, no census, and not an integer
   lever.
3. S1 runs the eight fixtures at k=3, paired, for about $0.61: say 23/24 against 23/24,
   the base's reading in row 0056. One missed trial is inside the class tolerance.
4. S2 runs one dev task and gets a measurable trial for about $0.11.
5. S3 screens dev at k=3, paired, against a fresh base. The median task-level graded
   difference is ≥0, and no pass is lost.
6. S4 runs validation on 12 tasks at k=2. That is access 1 of 4, at confidence 0.9875.
   Say the cost interval over the 12 per-task differences lies below 0, and at most one
   task's graded difference is ≤ −δ, so road 2 passes. The round opens a PR changing
   the pinned routing in the three workflows that `selftest.py` holds to one value
   (`evals/README.md:560-561`), with the ledger row and the trial rows.

The numbers in steps 1, 5 and 6 illustrate the shape of a result. They are not
predictions.

## 12. Limits

- **The gate is strict on purpose.** At 12 validation tasks and 4 accesses, it accepts
  only when at most one task fails to improve. Real but uneven gains, better on some
  tasks and worse on others, read inconclusive. That is the right failure direction for
  a self-modifying loop, and the price of honest small-N statistics.
- **Bonferroni is conservative.** A proper reusable-holdout mechanism (Thresholdout)
  would buy more accesses. That is ⏸ until the loop has produced verdicts.
- **A pinned upstream measures one serving stack.** Users on default routing get
  another. Promoting a pin to the product default is its own decision in the PR.
- **The census covers three predicates.** Others need the session to record their
  counters first.
- **The proposer may produce near-duplicates.** The hash catches only exact repeats, so
  a near-duplicate pays for S1.
- **Terminal-Bench content never enters git.** Trial rows carry task ids and numbers
  only, and the public mirror (D172) carries those ids.
- **$0.13 per trial is a censored median.** Trial costs are only as bounded as the
  watcher's caps.

## 13. Decisions log (verbatim)

Round 1:

- Budget: "$20/wk, $10/run"
- Models: "flash + upstream pinned (Recommended)"
- Held-out: "Budgeted reuse + frozen final (Recommended)"
- Text levers: "Candidates are commits (Recommended)"

Round 2:

- Score: "Graded partials, pass reported (Recommended)"
- Host: "Buildhost VM, 6+ slots (Recommended)"
- Human gate: "Owner starts a round; caps gate spend (Recommended)"
- Trial rows: "Commit rows, keep runs outside (Recommended)"

Round 3:

- The proposer's memory: "we need to build out a full memory system. this will be
  handled by a different agent. the solution for now is that the llm proposer can read
  past session jsonl. i would like to implement semantic search for past sessions"
- Final set: "6 fresh tbv4 band tasks (Recommended)"
- Loop levers: "Yes, via LoopConfig (Recommended)"

Round 4, after the review:

- Group size: "Dev 6 screens, validation 12 (Recommended)"
- Run cap: "Per stage (Recommended)"
- Issues: "Yes, update the four (Recommended)"
- Budget: "you can make the budget a soft and hard budget so we can comfoterbly fit 2 runs"

Round 5, 2026-09-27:

- Weekly caps: "$25 soft / $30 hard" (approved)
- Rows without an issue: "File issues, cite #N"

## 14. Open questions

1. Does semantic search over sessions supersede the memory plan's "No embeddings"
   (`docs/plans/2026-09-10-memory.md:983`), or is it scoped to the proposer's corpus?
   The memory agent owns the answer.
2. The access budget A = 4 per final draw: is that the right number? At about two
   validation verdicts a week, it is two weeks per final draw. That means N2's 6 final
   tasks are replaced often, and the 36-task pool lasts roughly six draws.
3. The proposer's lever surface is text assets, levers and config. No Rust.
4. Which Buildhost VM hosts harbor, and on which disk? Not the runner VM's CI runners, and never
   under `/tmp`, which is RAM on the Buildhost host.
5. Should the compaction trigger charge the cached prefix on small-window models (§1)?
   That changes design §4.4's deliberate scope, so it needs its own D-row and
   deterministic test, outside this program.
6. `extract.py`'s `denials` counts only "Permission denied" (`:631`). That matters once a
   Mac or auto-mode suite exists.

## 15. D-rows owed

Numbered at landing from the next free row on main; check open PRs for collisions.

- The task-level graded gate, the two roads, the censor rule and the reason codes
  (extends D220).
- The trial store with verdict rows absorbs D219's rejection memory; the validation
  access budget with its Bonferroni threshold.
- The loop guards become tunable through `LoopConfig` fields (amends the D220 manifest).
- E16; harbor through the sweep driver; the soft/hard weekly budget.
- The improvement round: the proposer container, the mount rule, the cascade.

## 16. Not building

- A population or Pareto-front optimizer (GEPA-style), until verdicts exist.
- A least-squares fit or a Gaussian process over the levers, which §10.4 forbids.
- A t-interval at the task level. `levers.py:181-184` rejected it for token counts, and
  the order-statistic interval over tasks is kept.
- An LLM judge in scoring. The task's own verifier is the only grader.
- A scheduled round, auto-merge, or auto-written memories.
- Cassette or mock scoring of behavior changes. Cassettes stay a correctness gate.
- SWE-Atlas QnA and DeepSWE runs now (zero ledger rows); ⏸.
- A separate verdict ledger, a separate census script, or a separate harbor driver.
- A sandbox latency microbench, and classifier evals (#587 has not landed).
- The semantic search engine itself, which belongs to the memory agent.

## 17. Review log (revision 2)

An adversarial review (Cursor, 2026-09-26) reported 1 blocker, 17 majors and 9 minors.
Each major was re-checked against the tree before it was accepted.

| finding | verdict | where it landed |
|---|---|---|
| Blocker: pairs pooled across repetitions; at 6 tasks, n=6 needs 6/6 | accepted; `levers.interval` on six positives gives 0.9688 (checked) | §6.3 task-level pairing; round 4: validation grows to 12 |
| δ equal to a t-test MDE | accepted | §6.3 δ from the N1 task-level distribution |
| Road 1 compared cost as a point | accepted | §6.3 cost interval |
| Dropped pairs hide blow-ups | accepted | §6.3 one-sided failure is a loss |
| Base reuse, uncorrected validation reuse | accepted | §5.3 fresh base; §6.3 Bonferroni over A=4 |
| ctrf fallback scores wrappers as 1.0 | accepted; `selftest.py:323-335` | §6 graded score, never ctrf |
| N1 drops tasks on a k=2 sample | accepted | §6.1 no drops |
| Budget arithmetic ($9.83, one-arm S1) | accepted | §5.3 recomputed ($11.66) |
| Prefix 3% mixed billed and catalog; tools omitted | accepted; 9.3% (computed) | §1, §4 T8 |
| "~30%" was the 95% case | accepted | §4 T1, §7 (~20% at 0.9) |
| Watcher censoring missing | accepted | §3.6, §6.3, §12 |
| Leakage via ledger, trials, PR history | accepted | §5.5 mount rule, no history |
| Census predicates unrecoverable; stale signals | accepted; `extract.py:326,419` and `todo/mod.rs:132` re-read | §6.5 three predicates; fixes |
| Baseline lock riding the code commit | accepted; `check_commit_style.py:151-154` | §5.3 S0 |
| Rationale waives the predictor | accepted | §5.3 S0, §6.7 |
| Dangling "§7 stage 0" for unpriced turns | accepted | §8 stage 0 |
| T4 evidence stale | accepted | §4 T4, §7 recount first |
| LoopConfig fields never read by the loop | accepted | §5.2, §8 stage 3 |
| Minor citation fixes (account.rs, memory store, retry, auto-review, cache key, README blocker, SD band, probe) | accepted | §1, §3, §6.1, §6.4 |
| Merge the verdict ledger, census and runner into existing pieces | accepted | §0, §9 |
