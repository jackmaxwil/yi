# Test Foundations — five classes, two mechanisms, one fired trigger

```
status:  REVISED 2026-08-28 after a five-scout codebase review; supersedes the
         same-day PROPOSED draft. Nothing landed. Steps carry their own D-rows
         and version bumps when they land; this document authorizes none of
         them. Three bug fixes found by the review are running as separate
         sessions (hashline UTF-8 panic, mapped_effort null fall-through,
         config strictness) and are not steps here.
date:    2026-08-28
sources: crates/ai/tests + src (scout-verified) · crates/*/tests inventory ·
         scripts/guardrails/* mechanics · YI_DESIGN.md §9/§12/§13/§15/§18/§19 ·
         ARCHITECTURE.md D17/D24/D42/D51, feature ledger · docs/TODOS.md J-rows ·
         .github/workflows/ci.yml · har-verify ladder ·
         the 2026-08-28 mandatory-reasoning HTTP 400 incident
```

The session that produced this hit a hard 400 from OpenRouter ("Reasoning is
mandatory for this endpoint and cannot be disabled") on a model the test suite
never exercised, fixed it, generalized the lesson into a taxonomy, and then
fanned five read-only scouts across the tree to pressure-test the resulting
plan. The scouts broke three steps of the first draft and found two live bugs.
This revision folds all of it in; §2 records what changed and why, because a
plan that hides its corrections teaches nothing.

## 1. Thesis

Yi's tests classify by **contract source** — who owns the ground truth a test
defends. Five classes:

- **C1 — wire conformance.** External byte contracts: Pi v4 JSONL, ACP frames,
  provider request/response shapes. Ground truth is another implementation's
  output, never Yi's. Home: crates/types/tests fixtures,
  crates/session/tests/conformance.rs. Exists, healthy.
- **C2 — external contract drift.** Facts Yi bakes in that live on someone
  else's server: the model catalog (`reasoning.mandatory`, effort maps,
  context windows, pricing), OpenRouter's request schema, ACP schema, MCP.
  C1 asserts we serialize what we meant; C2 asserts what we meant is still
  true. Cannot run in `just check` — network, flaky, rate-limited. **This is
  the missing class**; the 400 was its first invoice.
- **C3 — behavior under faux.** Deterministic, keyless, scripted-stream
  plumbing: loop, tools, permission, compaction, subagent recursion, schedule.
  Home: `crates/*/tests/*_faux.rs`, `*_e2e.rs`. Largest class. Mostly healthy,
  with two thin spots the inventory scout measured (§4.3).
- **C4 — surface drive.** Real loop, real screen/protocol, scripted input:
  `yi tui --headless`, scripts/tui_pty.py (manual-only, by doctrine),
  rpc/acp/cli protocol tests. Exists.
- **C5 — evals.** Task outcome and cost, not plumbing: pass/fail on fixture
  repos, tokens per turn, cache-hit ratio, turns, wall time. J1–J3 open,
  `evals/` does not exist. Nightly, never in `just check` (§12 settles this —
  see §2 item 3).

Guardrails are not a sixth class. They are a reporting mechanism — but only
where the metric is deterministic; C2 diffs are reviewed artifacts, not
ratchets, and C5 metrics ride §9's existing 2%-band token ratchet, not the
shrink-only kind (§4.4).

Two axes the taxonomy deliberately does not carry, now named so they are
rejected by choice: **verification strength** (har-verify's ladder — §18
already mandates its rung 4 for three parsers, so that hole is a step, §4.5)
and **resource bounds** (no class defends against unbounded input; the scout
findings that make this concrete are a step, §4.3).

## 2. What the scout review changed

The first draft was pressure-tested against prose; this revision is
pressure-tested against the tree. Five corrections, each of which reversed or
rewrote a step:

1. **Root-level `evals/` would be invisible to every guardrail.** Every
   guardrail glob is hard-coded to `crates/*` — naming law
   (check_manifests.py), boundaries, panic budget, file/fn size, comments,
   duplication, env surface. A workspace member at the repo root passes by
   never being enumerated: its code would ship exempt from the zero panic
   budget. The draft's claim that landing evals "touches the naming law and
   boundaries.toml" was intent, not mechanism. §4.4 now puts the crate at
   `crates/evals` (`yi-evals`) where the law already binds, and the D-row
   that lands it also resolves the standing doc conflict (§12 calls `evals/`
   a workspace member; ARCHITECTURE.md's non-crate list files it as a
   directory).
2. **The draft's nightly rejection contradicted settled design.** J1's own
   done-condition is "nightly runner"; §12's adoption row says the suite is
   "run nightly, not per-PR". A plan paragraph cannot revise that — the
   workflow rules require a D-row first, and none is proposed. Reversed:
   §4.4 honors the design. The interim local-before-refresh discipline for
   the drift script (§4.1) stands on its own and never conflicted.
3. **Cassettes have a designed home.** §12 places them at `evals/cassettes/`
   — inside the container J1 creates — and §9's token ratchet and
   cache-prefix guardrail both consume them, which means those two §9 rows
   are unimplementable until J3 lands. The draft's "storage beside existing
   fixture conventions" and its cassettes-before-evals ordering both
   deviated unflagged. Folded into §4.4 as the evals step's first phase.
4. **The fuzz deferral's premise died on inspection.** The draft deferred
   fuzzing until "the first parse-path panic"; the parser scout found one
   the same day — a reachable char-boundary panic in `Patch::parse`
   (hashline input header noise-stripping slices a lowercased string with a
   byte index computed against the original; a width-changing lowercase
   mapping between keyword and colon panics). The trigger fired before the
   plan landed, and §18 was already normative: fuzz targets for the hashline,
   cron, and ACP-frame parsers, with the ARCHITECTURE rubric claiming them
   "from phase 0". Deferral was a design deviation dressed as prudence.
   §4.5 is now a real step. (The panic fix itself is in flight as a separate
   session; the fuzz target is the mechanism that finds the next one.)
5. **The offline matrix would have checked dead data.** Only four compat
   keys have a reader anywhere in the workspace (`thinkingFormat`,
   `supportsDeveloperRole`, `requiresReasoningContentOnAssistantMessages`,
   `supportsTemperature`); nine others, `Model.headers`, and openrouter-side
   cost tiers are write-only. A C2 script or matrix that verifies unread
   keys is waste. §4.1/§4.2 now scope both to the read-set — and §4.2 gains
   a row for the second live bug the scout found (`mapped_effort` sends an
   unsupported effort verbatim when the catalog maps a non-off level to
   null; fix in flight, the matrix row is the regression guard).

Two facts from the pre-scout pressure test carry over unchanged: the matrix
is not greenfield (crates/ai/tests/openrouter.rs already holds the
mandatory-reasoning and effort-mapping seed rows), and J8 closed at 0.31.0
(D51) — cited as a past incident of the silent-cost class, not open work.

## 3. Pipelines, by blast radius

Ordering principle: cover the pipeline whose failure costs the most, first.

1. **Request assembly** — context → provider params. Every turn pays it;
   failures are hard 400s (this incident, and the latent null-effort bug) or
   silent cost (J8's class). Three builders: openai-completions, anthropic,
   openai-responses; dispatch on `model.api` in yi-runtime. 71 of 350
   catalog models take the mandatory-reasoning branch; ~23 openai models map
   `max` to null.
2. **Stream mapping** — SSE bytes → events → JSONL. The scout found the seam
   the draft missed: every mapper test bypasses `SseDecoder` entirely
   (hand-built JSON values pushed straight at the mapper), so no test feeds
   real chunk boundaries — split UTF-8, CRLF, keep-alive comments — through
   the byte→event→message chain. Cassettes only close this if recorded as
   raw SSE bytes, not events (§4.4).
3. **Session persistence and resume** — Pi interop, fixture-locked, good.
   One caveat the parser scout surfaced: the load path *mutates* on
   malformed input — a torn tail on load rewrites the user's file in place,
   dropping the partial line. Deliberate repair, but it deserves a property
   test that the rewrite never loses a complete entry (§4.3).
4. **Tool execution + permission + wall** — deny gates proven by neutering
   the gate; that discipline is settled. But the draft's "fine, keep it" was
   too rosy: the wall — the whole instrument-write barrier — has exactly two
   tests (§4.3).
5. **Turn control** — interrupt, compaction, heartbeat. Interrupt has one
   faux mid-stream case and nothing during a tool call, during compaction,
   or with a child running (§4.3).
6. **TUI render** — frames + PTY, covered by the phase-7 doctrine.

## 4. The plan

### Step 1 — C2: split the drift script (S)

scripts/openrouter_reasoning.py today has **no CLI surface at all** — no
argparse, no exit discipline; `main()` unconditionally fetches
`https://openrouter.ai/api/v1/models` and rewrites
crates/ai/data/openrouter.json in place. Split it to match the guardrail
convention (`_common.fail`-style verdict, nonzero exit on drift): default
invocation fetches, recomputes what the catalog should say, diffs, reports;
`--update` writes. A diff is a reviewed artifact, never an auto-commit.
Scope the recomputation to the read-set (§2 item 5): `reasoning`,
`thinkingLevelMap`, and the four consumed compat keys — not fields nothing
reads.

Add a `just conformance` recipe pointing at the script where it lives. No
`scripts/conformance/` directory yet — one member does not need a directory;
the second script creates it. Run discipline until the evals nightly exists
(§4.4): `just conformance` runs locally before any catalog refresh is
committed. The endpoint is unauthenticated, so the eventual cron needs no
secrets.

### Step 2 — C2 offline half: model-class tables in crates/ai/tests (M)

Generalize the seeded openrouter tests into table-driven coverage over
representative catalog entries, one row per compat class, asserting the
`build_params` output the class requires. The house table idiom to reuse is
crates/session/tests/conformance.rs's `for_each_backend` closure — axis as
data, case body written once. Rows, per provider file:

- openrouter (openai-completions): mandatory-reasoning omission (`off: null`),
  effort mapped through `thinkingLevelMap`, effort passed through unmapped,
  non-reasoning model emits no reasoning key, `supportsDeveloperRole: false`
  keeps the system role, `requiresReasoningContentOnAssistantMessages` fills
  the field.
- openai-responses: the same effort classes **plus a null-on-non-off row**
  (`max: null` and kin) asserting the fixed clamp/omit behavior — the
  regression guard for the in-flight `mapped_effort` fix.
- anthropic: adaptive vs budget thinking shapes, cache_control placement
  (already covered; fold into the table only where it shrinks the file).

Per-provider tables, not one cross-provider matrix — `build_params` shapes
differ per provider, and a single matrix spanning three builders is a shadow
model of provider dispatch. Two code-side facts the matrix cannot check but
this step records for the ledger: `strict: false` is hardcoded in all three
`convert_tools` while the catalog advertises strict-mode support on 51
models (dormant capability, not drift), and the `mapped_effort` helper is
duplicated between openai.rs and openai_responses.rs (fold when the fix
session touches both).

Layering, stated once: the table's ground truth is the baked catalog, which
is only true while Step 1's script says so. The offline matrix without the
drift check still 400s the day OpenRouter changes; they are one contract
split at the artifact boundary.

### Step 3 — bounds and thin gates (M)

The scout review found the places where the existing classes are thin enough
to have hidden real defects. Small, targeted, all offline:

1. **SSE resource bounds.** crates/ai's byte pump accumulates into an
   unbounded buffer — one invalid UTF-8 byte early and the stream never
   re-syncs, growing to the whole response in memory — and the SSE decoder
   has no event-size cap, while crates/mcp-cli already caps at 16 MiB. Bound
   both (mirror the mcp-cli constant), with tests that a poisoned stream
   errors instead of ballooning.
2. **Torn-tail property test.** The session load path's in-place repair
   (pipeline 3) gets a property test: for any Pi-valid file arbitrarily
   truncated mid-final-line, repair drops only the torn line and every
   complete entry survives byte-identically.
3. **Wall rows.** Two tests guard the whole instrument-write barrier today.
   Add the missing directions: wall deny on a read-class tool, overlay
   reduction on a child, wall interaction with yolo mode — each proven by
   neutering the gate, per the settled discipline.
4. **Interrupt rows.** Add the three absent cases: interrupt during a tool
   call, during compaction, with a child running.

Item 1 is a behavior change (new error on pathological streams) and carries
a feature-ledger line; 2–4 are pure test additions against the test-LOC
budget.

### Step 4 — C5: evals at crates/evals (J1/J2/J3, one container) (L)

`crates/evals` as `yi-evals`, where the naming law, boundaries allowlist,
panic budget, and size gates already bind — not a root-level directory the
guardrail globs never see (§2 item 1). The D-row that lands it records the
placement, resolves the §12-vs-ARCHITECTURE classification conflict, and
supersedes nothing else. Landing order inside the step:

1. **Cassettes first (J3), recorded as raw SSE bytes** at the §12-designated
   `cassettes/` home. Byte-level recording is what lets replay drive
   `SseDecoder` → mapper end to end and close pipeline 2's untested seam;
   event-level cassettes would fossilize the bypass. Recording needs keys
   and network once; replay is offline forever. Cassettes unlock the two §9
   rows that are currently unimplementable: the per-scenario token ratchet
   (>2% growth without `--update` — §9's own tolerance band, which is why
   this plan invents no new mechanism) and cache-prefix stability across
   cassette turns (J4/J5's `check_request_budget.py` is the acknowledged
   narrower stand-in until then).
2. **Runner (J1)**: fixture repos driven through `yi ask --json`, metric
   baselines as JSON under the existing baselines directory. Metrics split
   by ground truth: faux/cassette-groundable (turns, tool-call counts,
   assembled-context size, prompt tokens) versus real-provider-only (billed
   tokens, cache-hit ratio) — separate baselines, so a keyless run never
   reports a vacuous cost metric as green.
3. **Nightly cron** in ci.yml running the evals suite and `just conformance`
   — per J1's done-condition and §12's "nightly, not per-PR", which are
   settled design this plan follows rather than re-litigates (§2 item 2).
4. **Adapters (J2)** last: harbor and pier per D24/§15.5, the eventual
   consumers, ~250 lines of Python, E1–E9 gates.

### Step 5 — fuzz the three §18 parsers (M)

§18 mandates fuzz targets for exactly three parsers — hashline patch, cron
schedule, ACP frames — and the trigger the first draft was waiting for has
already fired (§2 item 4). Order by evidence: `Patch::parse` first (a panic
was found by reading; the fuzzer's job is the next one — the tokenizer is
full of byte-index slicing that happens to be ASCII-safe today), then ACP
frame decode (remote-adjacent input), then cron parse. Structured targets
where the grammar allows (fuzz the op type, not raw bytes), corpus and crash
artifacts committed as regression tests per har-verify rung 4.

Mechanics this costs, named so the landing commit is honest: cargo-fuzz's
`libfuzzer-sys` is a §13.3 table edit plus size-ledger entry (dev-scoped;
the transitive budget counts normal edges only), the fuzz harness crate
lives outside the workspace members list in the standard cargo-fuzz layout,
and `proptest-derive` is banned by §13.5's proc-macro clause — property
tests, where added, use the declarative macro only. Fuzz runs are timeboxed
beside `just conformance`, never in `just check`. §18's weekly miri row
stays dormant: every crate is `forbid(unsafe_code)` and miri buys nothing
until an unsafe dependency reaches a hot path.

### Step 6 — doc and convention hygiene (S)

Small ledger debts the scouts surfaced, batched into the Step 4 D-row commit
where they touch the same files:

- ARCHITECTURE's feature ledger cites a `J7` that has no TODOS row; §11's
  phase table has no 3b row though the ladder and §15.5 both cite 3b.
- The `.ruler` guardrails text claims a mixed baseline+code commit "fails
  the build"; nothing implements that — it is convention only. Either
  correct the prose or add the check; do not leave a false enforcement
  claim standing.
- Kernel wire frames compute an HMAC on send that is never verified on
  receive. Local ZMQ, low blast radius, but the dormant signature is either
  verified or its purpose documented; noted here so the decision is made
  deliberately rather than by archaeology.

## 5. Negative space — considered and rejected

- **`yi-testkit` shared test crate.** A 14th crate and a boundaries edit for
  helper reuse nobody has measured. Add when duplication fires on test code.
- **One cross-provider compat matrix.** Shadow model of provider dispatch;
  per-provider tables instead (§4.2).
- **`scripts/conformance/` directory now.** The second script creates it.
- **ACP/MCP drift scripts now.** Each baked artifact earns its script by
  drifting once. OpenRouter paid with a 400; ACP and MCP have not.
- **C2 coverage of unread catalog keys.** Nine compat keys, `Model.headers`,
  and openrouter tiers have no reader; verifying them is waste. If a reader
  lands, its key joins the checked set in the same change.
- **Root-level `evals/`.** Guardrail-invisible by glob (§2 item 1).
- **Event-level cassettes.** Would fossilize the SseDecoder bypass (§4.4).
- **Skipping the nightly.** The first draft rejected it; that rejection is
  itself rejected — it contradicted J1 and §12 without a D-row (§2 item 2).
- **Weekly miri now.** Dormant until unsafe exists on a hot path (§4.5).
- **Shrink-only ratchets on stochastic eval metrics.** §9's token ratchet is
  already a 2% band; new metrics follow that mechanism, not the LOC-style
  shrink-only kind. A D-row reconciles the framing when Step 4 lands.

## 6. Sequencing and cost

Steps 1–2 are this week's work: small, offline, and they close the incident
class that already fired twice (the 400, and the latent null-effort bug in
the same branch family). Step 3 is independent of everything and can
interleave. Step 4 is the large item; its internal order (cassettes →
runner → nightly → adapters) front-loads the piece both C5 and pipeline 2
consume. Step 5 starts after the in-flight hashline panic fix lands, so the
fuzzer hunts the second bug rather than rediscovering the first. Step 6
rides Step 4's D-row commit.

Order: 1 → 2 → (3 anytime) → 4 → 5, 6 riding 4. Each landing step updates
TODOS J-rows and, where structural (Step 4's crate, Step 5's §13.3 edit),
bumps ARCHITECTURE.md with its own D-row in the same commit. In-flight
elsewhere, not steps here: the hashline UTF-8 panic fix, the `mapped_effort`
null fall-through fix, and the config strict-vs-lenient reconciliation —
three sessions spawned from the review that found them.
