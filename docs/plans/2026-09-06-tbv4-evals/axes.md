# The measurement design: five axes, every column from a file Yi already writes

Law: a column is a deterministic function of the run's own artifacts (the
harbor `result.json`, the `yi.jsonl` event stream, the v4 session JSONL
under `logs/agent/yi/sessions`, the `.telemetry.jsonl` sidecar). No judge, no
model, no scheduled pass (flywheel law 2, D54; prompt-surface law 1). A
column that cannot be defined this way is not a column.

Sources, by name:

- **signal** — a key of `SIGNAL_NAMES` in
  `skills/yi/session-mining/extract.py:255-262`, computed by `signals(entries)`
  (`:277-405`) from the session JSONL. Twenty exist at v4 (`EXTRACTOR_VERSION = 4`, `:26`).
- **session** — a field of `extract_session` (`extract.py:407-574`): `tokens`,
  `turns`, `peak`, `compactions`, `repeated`, `reads`, `first_mutation`,
  `interrupts`, `denials`, `failures`, `by_tool`.
- **emission** — a row of YI_DESIGN §15.4 as the adapters fill it:
  `yi_usage.parse_events` (`input`, `output`, `cacheRead`, `cacheWrite`,
  `costUsd`, `costUnknownTurns`, `nAssistantMessages`) and
  `yi_usage.session_extras` (`peak_context_tokens`, `summarization_count`,
  `n_agent_steps`).
- **harbor** — `TrialResult` (`harbor/models/trial/result.py:70-97`):
  `verifier_result.rewards`, `agent_execution` timing, `exception_info`.
- **telemetry** — `Span` (`crates/types/src/telemetry.rs:21-58`) lines in
  the sidecar, rolled by `yi stats telemetry <dir>` (D132).
- **custom** — a `custom{todo}` or `custom{todo_intercept}` entry the todo
  tool writes (D137; `crates/runtime/src/todo/coupling.rs:286-292`
  `record_intercept(store, rung, reason, fingerprint, total)`).
- **new** — a deterministic signal this plan adds to `extract.py`, defined
  here in full, each with a fixture seen red first (prompt-surface §8.1's rule).

## A. Pass (the index's own column)

| column | definition | source |
|---|---|---|
| reward | `verifier_result.rewards["reward"] > 0`; errored trial = 0 | harbor; `core/metrics.py:24-26` |
| pass@k | per task `1 - C(n-c,k)/C(n,k)`, averaged over tasks; a task with fewer than k trials skipped for that k | `core/metrics.py:52-74`, reimplemented in twenty stdlib lines (no harbor import under `evals/`) |
| ci95 | 1.96 × √(Σ p(1-p)/(k-1) / n²) × 100, `k ≥ 2` only | `core/metrics.py:29-49` |
| timed out | `exception_info.exception_type == "AgentTimeoutError"` | harbor |
| verifier unmeasured | `verifier_result` is null while `verifier` has a start time: the verifier's own timeout, or one harbor dropped under an earlier agent timeout (D173) | harbor `_run_verifier`, `_record_exception` |
| partials | `results.summary.passed` / `.tests` of `verifier/ctrf.json`; `partial_score` of `verifier/trace_results.json` (D173) | the task's verifier |

## B. Persistence (the user never asks twice)

| column | definition | source |
|---|---|---|
| open todos at stop | 1 when the last `custom{todo}` snapshot has a `pending` or `running` item and the final assistant `stopReason` is `stop` | signal `stopped_with_open_todos` |
| asked twice | count of adjacent user messages whose token-set Jaccard ≥ 0.8 (in a harbor trial there is one user message, so this is a journey column) | signal `asked_twice` |
| multi-step without todo | 1 when ≥ 3 mutating calls and no `todo` call | signal `multi_step_without_todo` |
| todo stale | 1 when ≥ 12 changes landed since the last state-changing todo op (`NUDGE_WORK`, `coupling.rs:18`) | signal `todo_stale` |
| intercept count | number of `custom{todo_intercept}` records with `reason == "open"` (the runtime re-drove a turn that ended with open work) | **new** `intercept_count`; reads `data.reason` of the entries `record_intercept` writes (`coupling.rs:286-292,431`) |
| intercept max rung | the largest `data.rung` among those records; 0 when none | **new** `intercept_max_rung`; same entries, `data.rung` |
| intercept capped | 1 when a record carries `reason == "let go"` (`INTERCEPT_CAP_PER_CYCLE = 6`, `coupling.rs:20,428`) | signal `intercept_capped` |
| waiting without block | 1 when the final message asks the user while an item is running and none is blocked on the user | signal `waiting_without_block` |
| blocked without question | 1 when an item is blocked on the user and the final message asks nothing | signal `blocked_on_user_without_question` |

A trial that passed with `intercept_count > 0` finished because the runtime
would not let it stop; a trial that failed with `stopped_with_open_todos`
stopped on its own. Both are visible per row; neither is a score.

## C. Rigor (software engineering discipline)

| column | definition | source |
|---|---|---|
| gate without change | gate commands (`cargo nextest`, `cargo test`, `just check`, `cargo clippy`, `GATE_WORDS` `extract.py:253`) run before any edit and any source read; v4 adds the task's own test runner words (`pytest`, `npm test`, `bun test`, `make test`, `go test`) to `GATE_WORDS` in the same commit | signal `gate_without_change` |
| done without evidence | `done` ops whose item carries no `evidence` | signal `done_without_check` |
| unsourced counts | numbers ≥ 3 digits in the final message that appear in no tool result and no user message | signal `count_claim` |
| gate rerun, unchanged tree | the same gate command twice with nothing landed between | signal `gate_rerun_unchanged_tree` |
| regression seen red | 1 when some test command (a `GATE_WORDS` match) returned a non-zero exit, then at least one `edit`/`write` landed, then the same command returned exit 0. Exact command match, order by call sequence. 0 otherwise, and blank when no test command ran at all | **new** `regression_seen_red`; from the ordered bash calls and their results, which `signals()` already indexes (`extract.py:328-334`) |
| flag error | results containing `unexpected argument` | signal `flag_error` |
| empty filter | results containing `0 tests run` | signal `empty_filter` |
| chain stop | a `&&` command whose result carries `exit code:` or `[chain stopped` | signal `chain_stop` |
| pointer never read | `[full output: <path>]` emitted, never read | signal `pointer_never_read` |
| self capped | `max_output_lines` set and nothing omitted | signal `self_capped` |
| denial as finding | `PermissionDenied` in a result and in the final message | signal `sandbox_denial_as_finding` |
| repeated calls | identical `(tool, args)` pairs | session `repeated` |

## D. Experience (what the person on the other side sees)

| column | definition | source |
|---|---|---|
| answer length | characters of the final assistant message; read against the request class (`assess`, `diagnose`, `change`, `monitor`, `ambiguous` from `evals/journeys/prompts/prompts.txt`'s first word; every v4 task is `change`) | signal `answer_shape` |
| closing offer | the last line ends with `?` or carries an offer form (`OFFER_WORDS`, `extract.py:254`) | signal `closing_offer` |
| cache-miss streak | the longest run of consecutive requests after the first with `cacheRead == 0` | signal `cache_miss_streak` |
| turns | assistant messages | emission `n_agent_steps` |
| wall | `agent_execution.finished_at - started_at` | harbor |
| ttft p50 | median `ttft_ms` over `request` spans | telemetry (needs the config line `yi-fit.md` names) |
| tool error rate | `tool` spans with `ok == false` over all `tool` spans | telemetry |
| reads before first mutation | `first_mutation.calls` (read-only calls before the first mutating one; `None` when nothing mutated) | session |
| interrupts | assistant messages with `stopReason == "aborted"` | session `interrupts` |

## E. Economy (speed, tokens, cost)

| column | definition | source |
|---|---|---|
| tokens in / cached / out | Σ `usage.input` / `usage.cacheRead` / `usage.output` over assistant `message_end` rows; `cacheWrite` kept beside them | emission `parse_events` |
| cache hit rate | `cacheRead / (input + cacheRead)`; blank when the denominator is 0 | derived from the same row (the formula `yi stats` uses, D116) |
| cost | Σ `usage.cost.total`; `?` when any turn is `usage.unknown` (never `-`, which means measured at zero) | emission `costUsd`, `costUnknownTurns`; `evals/run.py:ledger_row` already prints the two marks |
| cost per pass | cost / successes over the run; blank at 0 successes | derived |
| peak ctx | max over assistant messages of `input + cacheRead + cacheWrite` | emission `peak_context_tokens` |
| compactions | `compaction` entries | emission `summarization_count` |
| tokens per turn | (in + cached) / turns | derived |
| output per pass | Σ output / successes | derived; on QnA-shaped tasks output buys rubric coverage (§15.3 lever 10); on binary tasks it is pure cost |

## What a ledger row holds

The ledger keeps its fourteen columns and gains three on the right (the
plan's §"Ledger columns"). Each new cell is a compact triple so a row stays
one line:

- `persistence` = `open-at-stop / intercepts(max rung) / asked-twice`, e.g. `0 / 2(3) / 0`
- `rigor` = `gate-w/o-change / done-w/o-evidence / unsourced / seen-red`, e.g. `0 / 1 / 3 / 1`
- `experience` = `answer-chars / offer / miss-streak / ttft-p50`, e.g. `1840 / 0 / 2 / 1.9s`

The full per-trial table (every column above) is a JSON file beside the
run, named in the row's notes, the way `run.json` is for the live lane
(D133). The row is the summary; the file is the record.

## Where each axis can be measured

| axis | v4 trial (harbor) | Yi task (`evals/run.py`) | journey (`evals/journeys/ab.py`) |
|---|---|---|---|
| pass | reward, pass@k, ci95 | reward | none (a journey has no verifier) |
| persistence | everything but asked-twice | same | all, including asked-twice |
| rigor | all; `GATE_WORDS` needs the task's test runner | all | all |
| experience | all with the telemetry line; answer class is always `change` | all | all, with the class from `prompts.txt` |
| economy | all | all | all |

A v4 trial is the only place pass@k against a public row is meaningful. A
journey is the only place `asked_twice` and the request-class rows of
`answer_shape` mean anything. A Yi task in harbor's format runs in both the
harbor runner and `run.py` and is where the todo columns get a verifier
beside them.

## What is not measured, on purpose

- No LLM judge of quality, persistence or reward hacking. v4's `/judge`
  exists for the maintainers' leaderboard; on our ledger a trajectory is
  judged by the person who reads it, and the columns above tell them where
  to look.
- No composite score. Five axes, five groups of columns, one row. A single
  number would invite optimising the number.
- No new signal without a fixture seen red and an archived session it fires
  on (prompt-surface §8.1: "A fingerprint that fires on zero archived
  sessions is dropped").
