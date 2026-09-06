# The CI is the first user

Status: in progress. Step 1 (§3.A–B) landed as D131 (0.156.0) and step 2 (§3.E)
as D132 (0.157.0); the live-lane decision below is provisional D133; check the
number against open PRs before it is claimed. It amends plan law 3 of docs/plans/2026-08-29-governance.md (§4 here)
and leaves law 2 untouched.

Provenance: the eleven escapes of 2026-09-05, all found by the user opening yi
after every gate had passed (§1). The ask that followed: run a real, cheap
model in Forgejo CI on every pull request, with telemetry, metrics, capture,
correlation and observability that are fully automatic, so that time to first
token, cost, cache rate, tool ability and runtime error classes are measured
over time and regressions are caught before merge.

## The one-line design

**CI is the first user of every change: the real binary, daemon, terminal and a
real model, driven across every surface, instrumented everywhere, judged only by
deterministic checks, and ledgered.** Everything else here is the consequence of
that sentence: the harness must not touch the developer's environment, every
invariant must check and heal itself, every error must carry a class, every
number must have a band and a history, and landing must be a verb an agent
cannot get wrong.

## 1. What escaped, and the gate each one buys

Every gate that exists proves fixtures. Every escape below lives where the
fixtures end: in the environment, at a seam between surfaces, or in a number
nobody measured.

| # | escaped | class | why every gate missed it | gate that catches it |
|---|---|---|---|---|
| 1 | `openrouter/openai/gpt-6-astra` was `unknown model` for two days after OpenRouter listed it | stale data baked into the binary | the catalog was a build-time snapshot; nothing compared the bundle to the world | weekly drift job (§3.G); D128 already made the bundle a floor under a fetched cache |
| 2 | the refusal reached the console as `exit code 2` | error meaning dropped at a seam | tests asserted exit codes, never that the reason reached the client | refusal × surface matrix (§3.C) |
| 3 | three lanes left by headless drives filled the pool | verification leaked into the real environment; leak on abnormal exit; no self-heal | harnesses ran under the real HOME; nothing ever kills a session mid-run; an orphan waited for a human | harness isolation (§3.A); chaos journeys (§3.C); `yi doctor` after every run (§3.B); D130 fixed the symptom |
| 4 | boot resumed a deleted worktree's session, then `unknown session` | stale persisted reference plus a policy that trusted it | no fixture ever held a stale row | ledger invariant in `doctor`; boot policy test (0.155.0); daemon prunes rows whose cwd is gone (open) |
| 5 | a relative HOME planted a worktree beside the repo and the error named the wrong step | silent partial failure; wrong-cause error | no gate runs the binary under a hostile environment | harness refuses a relative HOME; each git step verifies its cwd before it spawns; an error names the precondition that failed |
| 6 | `keys: openrouter` breaks `yi tui`; the console runs | entry-point divergence | each surface has its own startup path and tests cover one | surface matrix (§3.C): the same config and the same refusal through ask, rpc, acp, tui, console |
| 7 | the console loop lacked the picker's logo tick | a feature wired into one loop | the loops are enumerated nowhere | the same matrix, as data: a feature names the loops it lives in |
| 8 | baselines conflicted on every merge; a ratchet commit swallowed code files | process friction, then hand edits, then mistakes | the ratchet mechanics are manual; `just commit` stages everything | merge driver for baselines; `just land` (§3.F) |
| 9 | the `title` job went red after open (a lost `Closes`, a missing milestone) | metadata checked after the fact | `pr open` checks; `pr edit` does not | `pr edit` re-runs `pr check`; issue creation demands a milestone |
| 10 | a socket path over `SUN_LEN` and a `wait-frame` timeout were the only signals | harness error masquerading as product failure | the harness has no failure class of its own | harness failures are `inconclusive`, never a red case (§3.D) |
| 11 | nothing in any gate measured time to first token, cost, cache rate or tool ability | zero runtime telemetry in gates | faux replays; a paid run is opt-in by plan law 3 | the live lane, the spans, the bands and the ledger (§3.C–E) |

"Never again" is not a property a build has; it is a property a process has.
The process here is the one the test doctrine already prescribes for code:
every escape becomes a gate, and the gate is seen red before it is trusted.
This plan extends that discipline from functions to environments, surfaces and
numbers.

## 2. What already exists (build on, do not rebuild)

| piece | where | reuse |
|---|---|---|
| task-eval runner over `yi ask --json`, scored by each task's `reward.sh`, spend caps, `--dry` refuses any non-faux model | `evals/run.py`, `evals/README.md` ("Budget discipline") | becomes the live lane's driver: `--live --cap-usd` is the one door money enters through |
| usage parse and config fingerprint, all-or-none token rule (E9), camelCase usage (E5) | `evals/adapters/yi_usage.py` | every live run is fingerprinted the same way |
| session JSONL → cassette recorder with the §10 redaction plant | `evals/record.py` | every live scenario can be re-recorded as a faux cassette for the T1 ratchet |
| behavior baseline, shrink-only (D76) | `scripts/guardrails/check_behavior.py`, `crates/runtime/tests/fixtures/behavior` | live cassettes feed it; the live lane never replaces it |
| `yi stats`: per-session `hitRate`, cost, turns, per-tool `durationMs` and error counts | `crates/cli/src/stats.rs` | gains a run-level mode over a telemetry file; the formulas stay the ones D116 fixed |
| `yi ask --json`: the timestamped event stream | `crates/cli/src/main.rs` (X5 exit-0 discipline, E1) | time to first token is the first `TextDelta` minus request start; the stream already carries both |
| real-binary journeys (T2), the exact `#[ignore]` reason as the tier marker | `crates/cli/tests/journeys.rs`, `scripts/guardrails/check_test_tiers.py` | chaos and refusal journeys join this tier; the marker discipline is unchanged |
| headless drives for the TUI and the console, the PTY harness with `--term xterm-kitty` and `--raw` | `yi tui --headless`, `crates/console/tests/drive.rs`, `scripts/tui_pty.py` | the surfaces the live lane drives; they gain an ephemeral HOME by default |
| one PR comment upserted by its first line | `scripts/forgejo_pr_comment.py`, the `size-report` job | the live report is a second comment through the same script |
| the postmerge lane and the weekly schedule | `.forgejo/workflows/postmerge.yml` (`just postmerge`, `just postmerge-evals`), `tracking-hygiene.yml` (cron `17 5 * * 1`) | postmerge appends the ledger row; the weekly job runs the matrix and the drift check |
| the eval ledger with additive columns and a run-id shared with the `Opt-Run:` trailer | `docs/eval-ledger.md` | the live lane's rows go here; new metrics are new columns on the right, as its own rule says |
| lanes, the pool, `yi lanes reap`, a work-free orphan freed on claim | `crates/runtime/src/lane` (D119–D124, D130) | `doctor` checks the invariants D120 states; the boot self-heal generalizes D130 |
| the daemon ledger beside the socket (D118) | `<socket>.ledger.json` | `doctor` verifies every row's cwd; pruning belongs to the daemon |
| the fetched catalog over the bundled floor (D128) | `yi catalog refresh` | CI's fresh HOME has no cache: the live job refreshes OpenRouter's public list first, which also exercises D128 |
| forge as the only register of work (D106), milestones, `Closes #N` | `scripts/forge_pr.py`, `check_pr_metadata.py` | every scenario that fails three times opens an issue under "Evals on the ledger" |

Nothing here needs a new crate, a new dependency, or a new store. The one
new file type is a telemetry JSONL that Yi already has every field for.

## 3. The system

### A. Harness isolation (three of the eleven; no model, no key, no decision)

Every run of the real binary — journeys, TUI and console drives, the PTY
harness, evals — gets an ephemeral **absolute** HOME and `--here`, unless the
scenario is about lanes. A relative HOME is refused with the reason (§1 #5).
`cli_surfaces`' `Workspace` already does this; `scripts/tui_pty.py` and the
console drive do not, and that is where the orphans came from. The developer's
`~/.yi` is never a test fixture.

### B. `yi doctor`: invariants as a table, healing where safe

One table the binary reads, closed vocabulary, one row per invariant:

| invariant | reads | safe repair (`--fix`) |
|---|---|---|
| every lane slot is a registered worktree, and every `yi/` worktree is a slot | `git worktree list --porcelain`, `~/.yi/lanes/<hash>/` | prune registrations whose path is gone |
| a `.held` under a free flock names no live session | `<n>.held`, `<n>.json` | none — D130's rule runs at claim |
| an orphan with a clean tree on a branch `main` contains is free | `git status --porcelain`, `merge-base --is-ancestor` | reap (what D130 does on claim) |
| every daemon-ledger row's cwd exists | `<socket>.ledger.json` | drop the row (the daemon's job; `doctor` reports until it does) |
| the socket is alive or absent, never a dead file | `connect` | remove the dead file |
| config parses strictly | `~/.yi/config.json` | none — name the key |
| the catalog cache is younger than `catalog.refreshHours` | `~/.yi/catalog/*.json` | none — report the age |
| HOME is absolute | env | none — refuse |

`doctor` runs a cheap subset at every boot (the self-heal), the whole table
after every live scenario, and in postmerge. A row that fails in CI is a red
scenario with the invariant's name as its error class.

### C. The live lane

A CI job `live` in `pr.yml`, beside `gate`, `title` and `size-report`. It builds
the debug binary, refreshes the OpenRouter catalog (public list, no key), and
runs scenarios through `evals/run.py --live` against
`openrouter/deepseek/deepseek-v4-flash-0731`.

A scenario is data, in `evals/fixtures/live/<name>.json`:

```json
{
  "surface": "acp",
  "prompt": "read README.md and append one line naming its first heading to NOTE.md",
  "requires": ["read", "edit"],
  "verify": "grep -q '^# ' NOTE.md",
  "budget_usd": 0.05,
  "timeout_s": 120,
  "lanes": false
}
```

`surface` is one of `ask`, `rpc`, `acp`, `tui`, `console`, `pty`; the runner
owns one driver per surface, the drivers that exist today. `requires` names
the tools the outcome should have needed; a run that reached the outcome by
another route is a pass with a note, never a fail — the verifier is the
contract, the tool list is the diagnostic. The first suite:

| ability | surface | verifier |
|---|---|---|
| read, edit (hashline) | acp | file content |
| bash | ask | a marker file the command writes |
| ipython (kernel boot) | rpc | cell output in the session file |
| fetch, walled | ask | the denial reason in the event stream, no egress |
| subagent spawn + merge | acp | the child's commit on the parent branch |
| plan create + `why` | rpc | `yi why` answers |
| undo | ask | the tree restored |
| lane claim + release (`lanes: true`) | ask | `yi lanes` idle afterwards, branch gone |
| `yi mcp --json` one-shot | ask | the tool's result in the transcript |
| compaction on a long context | ask | a `Compacted` event and the retained floor (P17) |
| permission deny | tui | the `△` row and no side effect |
| refusal matrix: unknown model, no key, pool full, config key, walled URL | every surface | the reason text on that surface |
| chaos: kill -9 mid-turn, then start again | ask, tui | `doctor` green, next start clean |
| chaos: daemon restart under an attached console | console | the sidebar repopulates from the ledger (D118) |

Verdicts are deterministic: the verifier's exit code, `doctor` after the run,
the error classes counted (§3.D), and the bands (§3.E). No model judges a
model — plan law 2, unchanged. A scenario whose verdict cannot be a shell check
is not a scenario.

Each scenario runs once per PR. Postmerge runs the suite once on `main`; the
weekly job runs it three times for variance. Money enters through one door:
`--live --cap-usd`, hard-stopped from measured usage (E9), $1 per PR run and
$5 per day, with the day's spend read back from the ledger before a run starts.

### D. Two vocabularies: error classes and run statuses

`ErrorClass` is a closed enum in `yi-types`, carried by every error event and
every refusal:

```
Refusal::{UnknownModel, NoKey, PoolFull, ConfigKey, Walled}
Transport::{Timeout, Status(u16), Proxy}
Provider::{RateLimit, Overloaded, BadRequest}
Tool::{Denied, Failed, Timeout}
Invariant::<doctor row>
Harness::{Socket, Timeout, Budget, Outage}
```

A wire-facing enum keeps `Other(String)` (§19). CI counts classes per run; a
class that appears on a PR and is absent from `main`'s last ten runs is red
with the class named. That is "identify critical runtime bugs" made mechanical.

A scenario ends in exactly one status: `pass`, `fail`, `inconclusive`. Only
`fail` is red. `inconclusive` is a harness class (a provider 5xx, a timeout at
the provider, the budget door, a socket path over `SUN_LEN`) after one retry;
its rate is a metric of its own. A scenario `inconclusive` three runs in a row
opens an issue under "Evals on the ledger" through the weekly tracking job —
the harness is a product too.

### E. Telemetry Yi owns

A `telemetry.jsonl` beside the session file, on by `telemetry.sink` in
config (a config key, not a `YI_*` variable: the env surface is capped at 40
and every row is a baseline). One line per span, correlation by id:

```
{"span":"request","session":…,"turn":…,"request":…,"provider":"openrouter","model":"…",
 "ttft_ms":812,"total_ms":4210,"input":5120,"output":388,"cache_read":4096,"cache_write":0,
 "cost_usd":0.0031,"retries":0,"fallback":null,"class":null}
{"span":"tool","session":…,"turn":…,"request":…,"call":…,"tool":"edit","ms":41,"ok":true,"class":null}
{"span":"lane"|"compaction"|"daemon"|"permission", …}
```

Every field but `ttft_ms` and the request span already exists somewhere in
the session record or the event stream; the span file gathers them under one
set of ids. `yi stats --run <dir>` rolls a run's files into `run.json`; a
script converts to SQLite on the ops host, and to OTLP only if a collector
ever exists there. No OpenTelemetry crate: it is absent from §13.3, the JSONL
converts outside the binary, and the binary's only new code is a writer.

Metrics the run reports, and their bands:

| metric | source | band (red) | ratchet against `main`'s last ten |
|---|---|---|---|
| time to first token, p50 / p95 | request spans | p50 > 3 s | +25 % |
| turn wall, p50 | request spans | — | +25 % |
| cost per scenario | usage (E9) | > `budget_usd` | +30 % |
| cache-read ratio | D116's formula | — | −10 points |
| tool success | tool spans | < 95 % | — |
| retries, transport fallbacks | request spans | fallback > 0 on a healthy run | — |
| error classes | §3.D | any class new to `main` | — |
| inconclusive rate | statuses | — | tracked, not gated |
| `doctor` rows | §3.B | any red | — |

Millisecond budgets stay local (D68): a shared runner cannot measure them.
Time to first token is seconds and provider-dominated, so a runner's jitter
is noise, and the run records the runner id and load so a band can be read
within one runner.

### F. Landing that cannot be done wrong

`just land` becomes the whole dance, in order: fetch; merge `origin/main`;
resolve every baseline by re-measuring (a `.gitattributes` merge driver for
`scripts/guardrails/baselines/*.json` that recomputes instead of conflicting);
fill the growth memo's number, never its prose; `just adr N` for a new row;
`pr open` or `pr update`; wait for the gate; merge; fast-forward. `pr edit`
re-runs `pr check`. Issue creation demands a milestone. Every commit verb
stages the diff's paths by name, never `-A`. Eight of the eleven escapes were
not code at all; they were an agent doing this by hand.

### G. The weekly job

On the existing schedule: the catalog drift check (bundle versus models.dev
and each provider's list, an issue when they differ), the adapter matrix
(one anthropic and one openai-responses model at tight caps, since the flash
model exercises only openai-completions), three runs of the live suite for
variance, the dependency audit. Per PR, the matrix also runs when the diff
touches `crates/ai/src/anthropic.rs` or `openai_responses.rs`.

## 4. The decision (provisional D133)

Plan law 3 reads: paid runs are opt-in, user-run, never in any gate. This plan
keeps that sentence for every tier it names and adds one tier beside T3:

| tier | what | where | spend |
|---|---|---|---|
| **T3-live** | the live suite, one real model, deterministic verdicts | `live` job on every PR (advisory, then required), postmerge, weekly | capped in code: $1/run, $5/day; the cap is a gate, the money is not |

Draft row: *the CI is the first user: a `live` job drives the real binary
across every surface against `openrouter/deepseek/deepseek-v4-flash-0731`,
judged only by verifiers, `doctor` and bands, capped at $1 per run and $5 per
day from measured usage, advisory until two weeks under a 5 % inconclusive
rate and required after; spans in `telemetry.jsonl` under `telemetry.sink`,
rows in docs/eval-ledger.md | every escape of 2026-09-05 was found by the user
opening yi after every gate passed, because every gate proves fixtures and the
bugs lived in the environment, at the seams between surfaces, and in numbers
nobody measured; plan law 3's "never in any gate" guarded against unbudgeted
spend, and a cap enforced in code guards the same thing with the run inside
the gate | delete the `live` job and the `--live` door; the suite stays
user-run under law 3 as written.*

Law 2 — no LLM anywhere in a gate's judgment — is unchanged and is the reason
§3.C's verdicts are shell checks.

## 5. Flow

A pull request: `gate` (lint, guardrails, test), `title`, `size-report` and
`live` run in parallel; `live` posts one comment — statuses, the class table,
the metrics with deltas against `main`'s rolling median, links to the
artifacts (session files, `telemetry.jsonl`, frames, raw PTY bytes) — and its
required status, once earned, gates the merge. Postmerge runs the suite once on
`main`, appends the ledger row with the run-id the `Opt-Run:` trailer already
shares, and comments the issues the merge closed as it does today. Weekly, §3.G.

## 6. Pressure test

- *Nondeterminism.* Verdicts judge outcomes and numbers, never text; one
  retry; provider-side failure is `inconclusive`; `live` is advisory until the
  inconclusive rate has been under 5 % for two weeks. A required status that is
  red for the weather gets ignored — the `title` job's history already shows it.
- *Cost.* OpenRouter lists `deepseek/deepseek-v4-flash-0731` at $0.05 per
  million input tokens and $0.10 per million output (2026-09-05), so a
  scenario of a few thousand tokens costs well under a cent and the fourteen
  together under $0.05 a PR; the $1 and $5 caps are two orders of magnitude of
  headroom, and a run that reaches them is a bug, not a bill. The cap is read
  from measured usage, and hitting it is `inconclusive` (budget), never red.
- *Secrets.* One repository secret, `OPENROUTER_API_KEY`; runner egress
  allow-listed to `openrouter.ai` and `models.dev`; the recorder's redaction
  already runs at record time, and the key is never an artifact.
- *One adapter.* Flash is openai-completions. The weekly matrix and the
  path-conditional per-PR run cover the other two.
- *Runner noise.* D68 stands; nothing under a second is gated on a shared
  runner, and every run records its runner.
- *A fresh HOME has no catalog cache.* The bundled floor already carries the
  CI model (checked 2026-09-05), so a fresh runner resolves it without a
  refresh; the job refreshes OpenRouter's public list anyway, which exercises
  D128, and the drift job says when the floor falls behind.
- *Cache metrics through OpenRouter.* D116's formula reads what the provider
  reports; a provider that reports no cache reads yields `n/a`, never zero, and
  the ledger column says which.
- *The LLM-judge temptation.* A scenario without a shell verifier is refused
  by the runner's schema.
- *Ledger growth.* One `run.json` of about 50 KB per run, artifacts for the
  rest; a year is twenty megabytes; `blob_size` is unchanged.
- *Wall time.* Eight to ten minutes: one kernel boot, three sessions in
  parallel, beside the other lanes rather than after them.

## 7. Order of work

Ranked by escapes caught per line, and by what needs neither a key nor the
decision:

1. **§3.A harness isolation and §3.B `doctor`** — no model, no key, no
   doctrine change; would have prevented #3, #4, #5 and the relative-HOME
   worktree outright. First.
2. **§3.E spans** — `ttft_ms`, the request span, `telemetry.sink`,
   `yi stats --run`. The measurement before the gate.
3. **§3.C the live lane, advisory**, eight scenarios: the refusal matrix on
   every surface, read/edit, bash, ipython, the two chaos rows. Needs D131 and
   the secret.
4. **§3.E bands and the comment, the ledger row in postmerge.**
5. **§3.D error classes** in `yi-types`, counted in CI.
6. **§3.F `just land` and the baseline merge driver.**
7. **§3.G the weekly matrix and drift.**
8. `live` becomes required when its inconclusive rate has earned it.

Each step lands as its own PR under the "Evals on the ledger" milestone, with
its gate seen red before it is trusted, as the test doctrine requires.

## 8. What this proposal deliberately does not do

- It does not judge output with a model, in any gate, ever (law 2).
- It does not replace the faux tiers: T0–T2 stay the fast, keyless, offline
  proof; the live lane records cassettes for them.
- It does not add a crate: no OpenTelemetry, no metrics library; a JSONL Yi
  owns and scripts outside the binary.
- It does not add a `YI_*` variable: telemetry is a config key.
- It does not gate sub-second timings on a shared runner (D68).
- It does not prune the daemon ledger from the console; that is the daemon's
  invariant, and `doctor` reports it until the daemon owns it.
- It does not promise "never again"; it promises that every escape becomes a
  gate, and names the eleven it starts with.

## 9. Open questions

1. OpenRouter prices cache reads on the flash routes (about $0.01 per million,
   2026-09-05), so the provider reports them; what remains to confirm at the
   first run is that the usage object carries the cached-token count D116's
   formula reads. If it does not, the cache column is `n/a` for the flash
   suite and the weekly matrix carries it.
2. Whether the runner's egress can be allow-listed at the host or only by the
   job; the wall (`deny_url`) covers the binary, not the harness.
3. Where the ops host keeps the SQLite rollup, and whether Grafana already
   exists there to read it; nothing in the plan depends on the answer.
4. Whether `live` should also run the console's true-terminal path under
   `xterm-kitty` on Linux runners, or only the headless drive; the PTY harness
   runs there, the placement counts are what today's picker work verified by
   hand.
