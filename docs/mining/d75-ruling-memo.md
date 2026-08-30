# Ruling memo: "D75" instrument-driven optimization (GEPA pilot, campaign 3)

Date: 2026-08-29 · branch `flywheel-3` · slice authorized $15.00 · spent **$0.00**
· reflection calls used **0 of 20** · candidates committed **0** · nothing promoted.

This memo decides nothing. It lays out the evidence for two rulings that are
the user's alone: (1) claiming the decision-log row the flywheel plan drafts as
"D75 — instrument-driven optimization", and (2) the YI_DESIGN §1.1
one-in-one-out ruling that S4/S5 triggers.

## 1. Pilot verdict: did not run, and correctly so

The pilot's own gate (dossier R2 §f, TODOS `J11`) is baseline rows on the T2
pair — the two easiest tasks that *passed* the TB2.1 6-task baseline. That
baseline does not exist, and the pilot's every rollout and reflection call
needs a provider key that is not set. Measured this session, in order of
severity:

1. `OPENROUTER_API_KEY` is UNSET (checked by name only, `[ -n "$..." ]`;
   `auth.rs` is env-only). Zero paid calls are possible.
2. No TB2.1 baseline row: `docs/eval-ledger.md` run 0001 is
   `fixtures@375d9b1` (3/3, $0.00098) — its own notes record "Not TB2.1:
   docker daemon down, harbor not installed, musl target absent." Docker is
   still down and harbor still missing this session. Runs 0002/0003 are $0
   read-only censuses. **The T2 pair is therefore undefined** — there is
   nothing to optimize against.
3. Variance is likewise unmeasured (no repeated-config row exists), so even a
   repurposed pilot would have no way to tell a candidate delta from noise.
   The instruction "if baseline variance makes the pilot unreadable, spend
   nothing" applies a fortiori: the baseline is not noisy, it is absent.

The $0 gates were run and are green before this verdict was written:
`evals/selftest.py` ok (9 checks), `evals/run.py --dry` ok (3 tasks), both
judged by exit code 0.

**Frontier-headroom test: NOT RUN.** `J11`'s kill rule ("no headroom the
optimizer could reach → GEPA is dropped, not tuned") takes a ledger as input.
That input does not exist, so GEPA is neither validated nor killed by this
pilot. It stays parked exactly where `J11` parks it. The one real-model row
that does exist (0001, fixtures, 3/3 at under a millidollar) has no headroom,
but the fixtures suite was never the pilot's instrument — reading it as a
headroom verdict on TB2.1 would be the vibe `J11` exists to stop.

## 2. What a run would cost (numbers, not prediction dressed as fact)

The only measured real-model datum: run 0001, three fixture tasks, 4 turns,
$0.00098 total, catalog cross-check 0% discrepancy (rates 0.075/0.25/0.015
per M in `crates/ai/data/openrouter.json`). Scaling that to the pilot shape
(≤20 reflection calls + T2-pair baseline replays + candidate re-runs, each
task ≤900s) puts realistic API spend in low single-digit dollars against the
$15 slice. The binding budgets are wall clock, docker image builds, and three
human preconditions — not USD.

Unblock path, in order: export `OPENROUTER_API_KEY` · start the Docker
daemon · pip-install harbor · `just package-musl` · run
`evals/drivers/tb21_baseline.sh` (exists, keyless-refusal verified) → row for
`tb21-6@<ref sha>` → T2 pair defined → this pilot becomes runnable inside its
slice.

## 3. Ruling A — the decision-log row (user's call)

The plan drafts the text as "**D75 — instrument-driven optimization**
(campaigns + GEPA, parked until S2 baselines; candidates are commits; exit
gate = zero suite regressions)". The literal number D75 is **already
consumed**: the flywheel landing at ARCHITECTURE 0.70.0 spent it on task
checks, and the header read this session shows 0.77.0. Any claim renumbers
against a freshly read header at claim time — sibling sessions are live and
have collided twice before (0.35.0/D55, 0.38.0/D57).

Options:

- **A1 — claim now, on today's evidence.** Against: the row would be written
  from zero TB2.1 baselines — `J11`'s own text calls a decision with no
  measurement behind it "the thing this plan exists to stop".
- **A2 — claim after run 0001-tb21 exists** (the row cites a real baseline;
  the pilot then runs inside the already-authorized slice). This is what
  `J11` and the plan both describe.
- **A3 — never claim; drop GEPA at the headroom kill.** Only available once
  the headroom test has an input; today it would be killing on no evidence,
  the mirror image of A1's error.

## 4. Ruling B — YI_DESIGN §1.1 one-in-one-out (user's call)

The plan's own open-calls list says it: "S0–S3 build the repo's own gates;
S4/S5 is where the §1.1 ruling bites — user's call at D75/D77 time."
Instrument-driven optimization as a top-level feature requires deleting or
demoting one in the same commit. This memo does not nominate a victim; the
inputs the ruling needs are the same baseline rows Ruling A needs, plus the
user's sense of which existing §1.1 row has earned the least. Deferring
Ruling B to the same moment as A2 costs nothing and keeps both rulings on one
evidence set.

## 5. Conflicts and risks recorded

- Memo path: the R2 dossier named `docs/plans/2026-08-29-gepa-pilot-memo.md`;
  the campaign-3 tasking names `docs/mining/d75-ruling-memo.md`. The direct
  tasking wins; this file is the memo, no second copy exists.
- Pilot asset: `crates/runtime/src/prompts/orchestrate.md`, 91 lines, clean
  on `flywheel-3` — but dirty in a sibling session on the main tree. Any
  future candidate commit here risks a merge conflict at land time; recorded,
  not resolved.
- "Config default model" (brief) remains false on this machine
  (`~/.yi/config.json` has `models:null`); every driver passes
  `--model openrouter/z-ai/glm-5.3-flash` explicitly.
- No key value was printed or persisted; presence was checked by name only.
