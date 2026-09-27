# evals/ — task-eval adapters and runners

Pure Python, standard library only. No Cargo.toml, no workspace member, no
dependency of its own: harbor and pier are the host harness's installs and the
adapters import them; everything the self-test exercises is stdlib.

The targets are three benchmark suites run under two harnesses: Terminal-Bench
and SWE-Atlas QnA under harbor, DeepSWE under pier. ARC-AGI-3 runs on its own
scaffold (`arc/`).

```
adapters/yi_usage.py         run command, usage parse, pier's extra columns, fingerprint (shared, pure)
adapters/yi_harbor/agent.py  harbor BaseInstalledAgent subclass
adapters/yi_pier/agent.py    pier BaseInstalledAgent subclass (+ install spec, allowlist, extras)
selftest.py                  dry run: no docker, no API, no keys, no harness
run.py                       task runner over `yi ask --json`, scored by each task's reward
surface.py                   tool-surface loop: one rollout per scenario, scored by the refusals its sessions recorded
axes.py                      five axes and the ledger row from any directory of trials (D140)
atif.py                      session file -> ATIF-v1.7 trajectory
record.py                    session JSONL -> behavior cassette, redacted at record time
levers.py                    the levers manifest, floors and the two gates (D220)
levers/                      levers.json, default.json, floors.json; split.json (development, validation, final task groups)
graph/refine.py              offline refiner for the procedural graph (D219): proposals in, a held-out gate, rejection memory
orient_census.py             read-only census of route telemetry and get_context packets in session files
judge_replay.py              stage 0 judge replay over recorded sessions, read-only (D259); replay/ holds its prompts and schemas
rule_fires.py                labelled haystack lanes against what the rule matcher can see
journeys/ab.py               journey prompts under one prompt ref, scored by the session-mining extractor
drivers/                     harbor sweep drivers, spend and wall caps (drivers/README.md)
arc/yi_arc.py                ARC-AGI-3 bridge: one `yi ask --json --yolo` per action (arc/README.md)
fixtures/                    recorded transcripts, v4 session files, runner tasks, live and surface scenarios
```

## Gates

Binary correctness gates on the path from a harness to a scored trial. A number
names one gate everywhere in code and tests; a number absent here is not a gate.

| # | Gate | Where it holds |
|---|---|---|
| E1 | `yi ask --json` exits 0 unless the run produced no assistant text at all; under `--eval` a run with no final answer exits 0 and prints a `{"type":"no_answer"}` event | `crates/cli/src/ask.rs` |
| E2 | a configured proxy is applied or startup fails (see [Proxy egress](#proxy-egress-e2)) | `crates/ai/src/request.rs` |
| E5 | usage is read in the wire's camelCase: `input`, `output`, `cacheRead`, `cacheWrite`, `cost.total` | `yi_usage.TOKEN_KEYS` |
| E6 | the run command has no bare `grep`; the `message_update` filter is guarded (`\|\| [ $? -eq 1 ]`), line-buffered, and `tee -a` writes to the events file with its stdout to `/dev/null` | `yi_usage.run_command` |
| E7 | the instruction is one shell-quoted argv, multi-KB accepted | `yi_usage.run_command` |
| E8 | `--yolo`; also `--here` (D119), and `--deadline` when the trial has one | `yi_usage.run_command` |
| E9 | token columns land all together or all `None`; any `usage.unknown` turn (D79) makes `costUsd` `None` and counts in `costUnknownTurns` | `yi_usage.parse_events`, `session_extras` |
| E10 | every harbor trial writes an ATIF trajectory (see [ATIF](#atif-e10)) | `yi_harbor/agent.py` (`SUPPORTS_ATIF`) |
| E12 | the instruction's `You have 28800 seconds` names the trial's own share, `EVAL_TIMEOUT_MULT` × 28800 | `yi_usage.with_budget` |
| E13 | the Terminal-Bench dataset is pinned by digest | `drivers/tbv4_baseline.sh` |
| E14 | cost is the provider's self-reported `usage.cost.total`, never a local price table | `yi_usage.parse_events` |
| E15 | the adapter uploads harbor's certifi bundle and sets `SSL_CERT_FILE`, since a task image may ship no CA roots | `yi_harbor/agent.py` |

## Emission map

What the adapters report to the harness, and where each value comes from.

| Harness field | Source | Adapter |
|---|---|---|
| `n_input_tokens` | Σ assistant `message_end` `usage.input` + `usage.cacheRead` | both |
| `n_cache_tokens` | Σ `usage.cacheRead` | both |
| `n_output_tokens` | Σ `usage.output` | both |
| `cost_usd` | Σ `usage.cost.total` (E9, E14) | both |
| `n_agent_steps` | count of assistant messages in the trial's v4 session files | pier |
| `peak_context_tokens` | largest single assistant `input + cacheRead + cacheWrite` in the session files | pier |
| `summarization_count` | count of `compaction` entries in the session files | pier |
| `metadata` | `adapterVersion`, `malformedLines`, `configFingerprint`; harbor adds `timeoutMultiplier` and `byUpstream` (turns per OpenRouter upstream) | both |

`usage.cacheWrite` is summed but not reported; reasoning tokens are not emitted. Wall time is
the harness's own measurement.

## Adapters

Both adapters run the one command `yi_usage.run_command` builds:

```
yi ask --json --yolo --here --model <provider/model> --session-dir /logs/agent/yi/sessions \
  [--deadline N] [--continue] '<instruction>' 2>&1 </dev/null \
  | { stdbuf -oL grep -v '"type":"message_update"' || [ $? -eq 1 ]; } \
  | stdbuf -oL tee -a /logs/agent/yi.jsonl >/dev/null
```

and parse `/logs/agent/yi.jsonl` after the run with `yi_usage.parse_events`.

- **harbor** (`yi_harbor.agent:Yi`): install uploads `EVAL_BINARY` or curls `EVAL_BINARY_URL`
  to `/usr/local/bin/yi` and checks `yi --version`; writes the trial HOME's
  `~/.yi/config.json` (`yi_usage.eval_config`: telemetry on, D132, and `routing` from
  `EVAL_ROUTING`); uploads the CA bundle (E15); runs `yi doctor --fix --json` to build the
  kernel venv inside the install phase and prints any red `kernel-toolchain`/`kernel-boot`
  row. `SUPPORTS_RESUME` passes `--continue`; `MODEL_CONNECTION` passes provider keys
  through under their own names. The fingerprint's mode is `yolo`, `+t<mult>` and the
  routing label.
- **pier** (`yi_pier.agent:Yi`): the same command and parse, plus `install_spec()` (one
  root `InstallStep` curling `EVAL_BINARY_URL`, verified by `yi --version`),
  `network_allowlist()` (the provider's API host only) and the three session-file columns
  above.

## Self-test

```
python3 evals/selftest.py
```

Exit code is the gate, and `just check` runs it: `check_guardrails.sh` invokes
it beside `check_behavior.py` (D76), so a broken adapter fails the same lane Yi's
code answers to. It pins the run command (E1, E6, E7, E8), the camelCase usage
parse (E5), the all-or-none token rule (E9), corrupt-line tolerance, pier's
three extra columns, the budget sentence, the routing config and its one pinned
value across the three workflows that set it, the ATIF conversion, the axes
rows, the driver caps, the recorder (turn grouping with stub FIFO and argument
key order, the redaction plant, the refusals), the surface scenario schema, the
graph refiner and the levers manifest. Stdlib only, no docker, no key.

## Task runner

```
just postmerge-evals          # cargo build -p yi-cli, then the dry tier of run.py and surface.py
python3 evals/run.py --dry --binary target/debug/yi --model faux/faux-1
```

A task is a directory under `fixtures/tasks/<id>/`: `task.json`
(`id`, `dryReward`, `timeoutSec`), `prompt.txt`, `repo/` copied verbatim into a
throwaway workspace, and `reward.sh` run there with the workspace as its cwd.
A task in harbor's layout (`task.toml`, `instruction.md`, `environment/app/`,
`tests/test.sh`, `solution/solve.sh`) runs here too: the instruction is the
prompt, `environment/app` is the seed, and `tests/test.sh` is the reward with
`APP`, `TESTS` and `LOGS` pointed at the copy, the task's own `tests/` and a
scratch dir, so `harbor run -p evals/fixtures/tasks/<id>` and this runner score
the same script (a host without pytest borrows one through `uvx`).
The workspace is `<tmp>/repo`, and the runner's own files sit beside it as
`<tmp>/events.jsonl` and `<tmp>/.yi-sessions` — never inside the graded tree,
where a `git diff` or clean-tree reward would score them as part of the
solution and the agent could read back its own transcript. The runner asks once
with `--json --yolo --cwd <ws> --session-dir <tmp>/.yi-sessions` (plus `--eval`
when `YI_LEVERS` is set), writes the last assistant text to `<ws>/answer.txt`
when there is one (a rollout that answered nothing leaves no file), and scores
the task binary: the reward exits 0 or the task scored nothing. `block-on-user`
reads its question there. A timeout is a result, never a retry.

`--dry` runs faux only — it refuses any other provider, because a gate spends no
API budget — and compares each reward to the task's `dryReward`, which is what
the exit code reports. Faux echoes the prompt and never calls a tool, so the dry
tier pins runner mechanics, not task solving: `answer-echo` scores 1 off the
echo (a runner that skips the answer file would score every real QnA rollout 0)
and `edit-file` scores 0 because the workspace is untouched (a runner that
scores an untouched workspace 1 flatters every real rollout). The third,
`clean-workspace`, lists the graded tree and requires exactly the task's own
files plus `answer.txt`. The dry tier needs a built binary, so it runs in
`just postmerge-evals`, not `just check`.

Drop `--dry` and the runner prints one JSON row per task plus a ready-to-paste
`docs/eval-ledger.md` row with its config fingerprint. Pasting it is a human act,
and a real-model suite is user-run and budgeted. `--out` keeps sessions, events,
`row.json`, `run.json`, the verifier log and an ATIF `trajectory.json`.

`EVAL_ROUTING`, OpenRouter's provider object as JSON, goes verbatim into the
run HOME's `routing` config key for `evals/run.py` and the harbor adapter
(`yi_usage.eval_config`, the one writer) and rides the fingerprint's mode as
`+routing{…}`, so a routing A/B needs no rebuild; anything but a JSON object is
refused, and so is `--home` on the fixtures lane, whose config stays the caller's.

## Tool surface

```
python3 evals/surface.py --dry --binary target/debug/yi --model faux/faux-1
python3 evals/surface.py --binary target/debug/yi \
    --model openrouter/z-ai/glm-5.3-flash --cap-usd 3 --out runs/surface
```

Runs one-shot agents over the tool surface and counts what was refused, per
tool. The refusal rate per tool is the number the surface is judged by.

A scenario is one entry in `fixtures/surface/scenarios.json`: the `road` it
exercises, the `prompt`, the `seed` files the workspace starts with, an optional
`levers` object (which makes the run an `--eval` run under `YI_LEVERS`, D220),
and `clean`, what a run that met no friction looks like. A new road is a JSON
entry, never an edit to the runner.

Each rollout gets its own workspace and its own HOME, as `run.py` does, so the
caller's `~/.yi` is never touched and one scenario's plans and lanes never reach
the next. A timeout is a result, never a retry.

Scoring is the session-mining extractor's: `--out/mining/issues.jsonl` groups
every refusal by tool with its text and whether the session recovered, and
`mu.jsonl` carries the per-tool call counts, so the runner reads that store
rather than the sessions. `surface.json` is the machine row (per-tool calls,
refusals and rate, the spend, whether a cap stopped it), and the printed report
is what a human reads: each refusal with its count, its text, the model's own
words from the turn it was made in, and a blank `correct? ____`. Judging a
refusal is a reading act; the runner never guesses what a caller meant.

`--dry` is faux only and refuses any other provider. `selftest.py::check_surface`
covers the scenario schema and the census with no binary and no key. A
real-model run refuses without `OPENROUTER_API_KEY` and without `--cap-usd`,
naming the missing precondition and never a key value, and prints its
`docs/eval-ledger.md` row.

## Judge replay (D259)

```
python3 evals/judge_replay.py all --dry --model faux/faux-1
python3 evals/judge_replay.py extract --corpus yi:$HOME/.yi/sessions \
    --corpus claude:$HOME/.claude/projects --out runs/replay
python3 evals/judge_replay.py label --out runs/replay --model openrouter/<id> --cap-usd 3 --jobs 4
python3 evals/judge_replay.py judge --out runs/replay --model openrouter/<id> --arm self \
    --split fit --limit 200 --cap-usd 3 --jobs 4
python3 evals/judge_replay.py judge --out runs/replay --model openrouter/<id> --arm judge \
    --split fit --limit 200 --cap-usd 3 --jobs 4
python3 evals/judge_replay.py report --out runs/replay
python3 evals/judge_replay.py mark --out runs/replay <boundary-id> intent_loss
```

Stage 0 of `docs/plans/2026-09-24-seven-primitives.md`: asked at a boundary, with the session so
far and nothing after it, does a model predict the owner's first objection? The owner's evidence
(plan section 1.3) is about the working agent: "i ask ai if the plan is done and everything was
done correctly", then "immediately ai can recognize things were not done correctly". A boundary
is a human message whose nearest message ancestor on the session tree is an assistant turn end.
`extract` (free) writes `boundaries.jsonl`: the intent record (file, entry id and text of every
earlier human message on that tree path), the turn's final text, the next message and the
agent's reply to it. Conversations split 70/30 into `fit` and `held-out` by the hash of a file
name: Claude Code rewrites a resumed transcript into a new file under the same entry ids, so
files that share or bridge to an entry are one conversation and a boundary found in several
counts once. Extract once per `--out`: a grown corpus can move a conversation whose new resumed
file sorts first.

v1 scored near chance: balanced accuracy 0.54 and 0.50 for glm-5.3-flash on two prompts and 0.52
for glm-5.3 at high effort, each on 200 fit boundaries, and 0.67 for Opus 5.5 on 61 with 9
positives. Its judge read a summary (the owner's messages, the final text, one line per tool
call), its positives mixed lost intent with new requirements, and its ground truth was what the
owner noticed, so a judge that found an unnoticed flaw scored wrong. v2 changes the input, adds
the self arm, narrows the positives and scores a probability.

`judge` renders each prefix at call time, re-reading the source file with the same reader: the
conversation on the boundary's ancestor path up to the turn end, oldest first, the owner's
messages as `[u1]`..`[uN]`, the agent's text as `[agent]`, each call as `[call <name>]` with its
arguments, each result as `[result <name>]`. Claude Code hangs parallel calls and their results
off the path, so the prefix also holds every block of an assistant message on the path and the
result of every call shown; nothing at or after the boundary in file order is read. A source
whose owner messages on the path no longer match the intent record is an error row, not a call.
Two caps speak where they cut. `tool_chars=2000` keeps the head of each call argument and each
result, a `[…]` row naming kept of total. `prefix_chars=240000`, about 60k tokens at four chars a
token (the smallest target context is Opus 5.5's 1M), drops whole tool results oldest first,
then agent text and calls oldest first, both from before the last turn (the one being judged),
then that turn's own results oldest first; never an owner's message and never the last turn's
text or calls. One row on top names kept of total per kind, the last turn's results separately,
and the cap, and a `[… n cut: prefix_chars]` row marks each gap. A cut that saves
less than its row waits for a neighbour to go. When the owner's messages alone pass
`prefix_chars`, the oldest go first under v1's rule, `intent_chars=60000`, with its own row; on
the 2026-09-26 corpus they peaked at 216,798 chars.

`--arm` picks the framing over the same prefix. `self` (`replay/self.md`) tells the model the
session is its own and ends on the owner's check as the last user turn (`replay/check.md`, in the
owner's words); `judge` (`replay/judge.md`) is the outside intent judge. Both answer the schema
in `replay/answer.md`: a verdict, objections citing the owner's words, and `p_objection`, the
probability that the owner's next message objects. A run writes
`verdicts/<model>-<arm>-<sha256 of the system prompt and the check>[-<effort>].jsonl`, each row
carrying its arm, and resolves every quote to a UTF-8 byte range of the named message. A verdict
without a `p_objection` in [0, 1] is a call without a verdict.

`label` writes `labels-v2.jsonl` and never v1's `labels.jsonl`: `objected`, `check_revealed` or
`accepted`, read from the intent record, the turn's final text, the next message and the reply.
An `objected` row names its `kind`: `intent_loss` when it rests on words the owner already said,
cited as `rests_on` quotes resolved like the judge's, or `new_info` for a new requirement,
opinion or taste. An `intent_loss` whose `rests_on` resolves to no owner message is demoted to
`new_info`. `mark` writes the owner's overrides (`intent_loss`, `new_info`, `check_revealed`,
`accepted`) to `marks.jsonl`, which win. Positives are `intent_loss` and `check_revealed`;
`new_info` is unpredictable by construction, counted and never scored. `judge` takes positives
and negatives interleaved p, n, p, n in id order, so a cut by `--limit` or `--cap-usd` stays
balanced; v1's sample, sorted by id, front-loaded negatives.

`report` prints, per run, corpus and split: n, positive rate, the `new_info` count, citation
resolution, recall, specificity, balanced accuracy with a conversation bootstrap 95% interval,
AUROC of `p_objection` (Mann-Whitney by mean rank, ties counting half) with its own conversation
bootstrap interval, catch, check recall and cost; then both arms side by side and every run that
has touched held-out, so tuning on it shows. Catch is mechanical: an `intent_loss` positive is
caught when the verdict flags it and the owner messages its resolved citations name meet the
label's resolved `rests_on`; a `check_revealed` positive reports recall only. The gate line
reads PASS iff held-out citation resolution >= 0.95, the AUROC interval's lower bound > 0.5 and
no held-out call went unanswered. A v1 run on disk (no arm, no `p_objection`, `labels.jsonl`)
still reports, its AUROC and catch `n/a`. Tune on `fit`.

What counts as the owner, surveyed on 2026-09-26 over 437 Yi files and 225 top-level Claude
Code transcripts:

- Yi: a `user` message with `attribution: user`, Yi's own rule (D25) since 2026-09-01; before
  it, a `user` message but the runtime's notices `[subagent …]`, `<ipython_state_restored>` and
  `[host] request`. The rule matters: six later plan nudges carry no attribution and no known
  prefix, and read as the owner they sat in 16 of 72 boundaries. A child session (a `parentSessionId` header,
  or a file under `sub-*`: the corpus's `rlm-*/sub-*` children carry no header link) is skipped.
- Claude Code: 49,617 of 54,166 `user` entries are tool results. Skipped besides: `isMeta`
  (skill bodies, command caveats, image notes), `isCompactSummary`, an `origin` other than
  `human` (1,699 `<task-notification>` entries), slash commands (`<command-name>`,
  `<command-message>`: typed, but the text is the harness's template), `<local-command-stdout>`,
  `<create-pr-command>` (a button) and `[Request interrupted …]`. A leading `<system-reminder>`
  block (38 messages) is cut and the typed rest kept, its byte offset recorded. A prompt typed
  mid-turn is an `attachment` of type `queued_command` with `commandMode` `prompt` (163): the
  owner's words, in the intent record. Files under `subagents/` are skipped; `isSidechain`
  appears only there. Lines split at `\n` only: 13 typed messages carry a raw U+2028, where
  `splitlines` would cut the entry. Claude Code can rewrite a block further down under the same
  uuids (16 of 175 files): an entry keeps the place of its first line.
- A turn runs back from its end to the owner, or to a command, notice or summary right after a
  turn end; a notice mid-turn (a skill body, a finished task, a plan nudge) does not cut it.

The corpus that day: 71 Yi boundaries in 34 conversations (17 held-out), 1,168 Claude Code
boundaries in 125 conversations over 173 files (458 held-out). 13 Claude Code boundaries have
no earlier owner message on their path (a session opened from a compaction summary).

Each call is one `POST https://openrouter.ai/api/v1/chat/completions`, stdlib `urllib`, with
no tools: the phase's prompt under `replay/` as the system message (for `judge`, the arm's
prompt or `--prompt`, then `answer.md`), the rendered input as a user message and the self arm's
check as a second, `temperature` 0, `response_format` the phase's
schema under `replay/` as a strict `json_schema`, and `usage: {include: true}`. The model receives nothing but that input,
so blindness holds by construction. `--model` is `openrouter/<id>` and the key is
`OPENROUTER_API_KEY` from the environment, sent in a header and never on argv or in a row; a
real run refuses anything else by name. The reply parses as JSON, else as the first `{…}` in it
(models fence JSON); otherwise the row carries `error`, the reply's `content` and no answer,
which the gate counts. A call times out at 300 s and retries 429 and 5xx with backoff, three
times at most. A row records the model, the provider's model id, `input` (cached tokens
included, the provider's convention), `output`, `cached`, `costUsd` and `latencyMs`. Cost is the
provider's own `usage.cost` (E14): a reply without it, or a 401, 402, 403 or 404, which every
later call would repeat, stops the phase. A call in flight when `--cap-usd` trips still lands,
so a phase overshoots by at most `--jobs` - 1 calls. The labeller's intent record has its own
240,000-char cap (`LABEL_INTENT_CHARS`), so an objection resting on an old message is not
demoted for want of it; its row names the newest messages kept of the total.

The first call path was `yi ask --schema` under Seatbelt. The first paid label pass stopped at
99 of 1,241 calls ($0.37): 66 returned no label, because Yi's loop pushed a model that had
already answered the JSON on into tool calls (0 to 65 per label) and it ended on prose, and
Yi's system prompt and tool schemas rode every call, 29k input plus 77k cached on average.
An agent is the wrong instrument for a label or a verdict.

A real run is the owner's: capped, and ledgered in `docs/eval-ledger.md` with the model, the
prompt hash and the gate line before any claim cites it. `--dry` is faux only and makes no
request: each call's host-built JSON comes back in the provider's reply shape at no cost and
takes the real parse path, and the run checks that no file under the corpus changed; it rides
`just postmerge-evals`. `fixtures/replay/yi/` is one real faux session driven over `yi acp`
(prompt, prompt, `_yi/rewind` to the second, prompt; its `ext_state` entry omitted and its cwd
replaced) and two real `yi ask --confirm` runs, the second `--continue`d, whose refused edit
before any read made the router write its plan nudge (prompt slots and cwd scrubbed);
`fixtures/replay/claude/` is three real transcripts and one subagent file with every
text replaced and the structure kept, the entries read as typed by hand marked `typed <line>:`.
`selftest.py::check_judge_replay` runs `tests/test_judge_replay.py`.

## Axes (D140)

```
python3 evals/axes.py runs/tbv4 --suite tbv4@39d9f44b --model openrouter/z-ai/glm-5.3-flash
python3 evals/axes.py evals/fixtures/axes --json /tmp/rows.jsonl
```

One JSON row per v4 session file found under the directory, its context the
nearest ancestor holding a harbor `result.json` (reward, wall, timeout) or a
run.py `row.json`, else a journey; then the `docs/eval-ledger.md` row with
the `persistence`, `rigor` and `experience` triples, `timeouts`, `partials`
and `upstreams` on the right (D173). The signals come from
`skills/yi/session-mining/extract.py` by import, the telemetry columns from the
`.telemetry.jsonl` sidecar beside each session. Exit 2 when a trial is
unmeasurable (a turn without usage, no assistant message, no session at all).
`evals/fixtures/axes/` holds one trial of each shape, plus a harbor trial that
timed out with its verifier past its ceiling, and `expected.jsonl` pins the rows
byte-for-byte (`check_axes`).

## ATIF (E10)

```
python3 evals/atif.py <session.jsonl> --agent-version $(yi --version | cut -d' ' -f2) > trajectory.json
```

The session file as an ATIF-v1.7 trajectory: one step per user and assistant
message on the main lane, the tool results that follow an assistant message as
its observation, metrics in harbor's convention (prompt tokens include cache
reads), final metrics summed and left unpriced when any turn reported no usage.
The harbor adapter writes `trajectory.json` into the trial's `/logs/agent` after
every run, and `run.py --out` writes one beside `events.jsonl`, so `harbor view`
can read a Yi trial. `evals/fixtures/atif/tool-turn.trajectory.json` pins the
conversion (`check_atif`).

## Cassette recorder

```
python3 evals/record.py <session.jsonl> --id recorded-0002-<slug> \
    --description "<what the case defends>" \
    --out crates/runtime/tests/fixtures/behavior/recorded-0002-<slug>.json
```

The v4 session file already holds everything a cassette needs, so recording is a
post-hoc read of a file Yi wrote on its own: the assistant entry **is** the
provider response (text, tool calls with their argument key order, usage) and
the toolResult entry **is** the tool result. No CLI flag, no env var, no capture
layer in the binary.

Main-lane message entries in `seq` order become the cassette: a `user` entry
opens a turn, each `assistant` entry appends a response spec, and each
`toolResult` appends to `stubs[toolName].results` in arrival order — per-tool
FIFO, which is exactly the order `StubTool` replays them in. Thinking blocks are
dropped with a printed note; `custom`, `model_change`, `thinking_level_change`
and `active_tools_change` entries are skipped with a note, because the recorded
responses already carry whatever they did. Everything else is **fatal at exit 2
with the reason named** — a compaction or branch summary rewrites the context a
linear cassette cannot reproduce, a non-main lane is not the conversation, a
corrupt line means the session was read only in part.

Redaction runs at record time, before the artifact exists: `record.py` imports
`redact` from `skills/yi/session-mining/extract.py`, so cassettes and mining
rows mask by one vocabulary. `fixtures/session-record/faux-echo.jsonl` carries a
planted fake `api_key=` that must come out `[MASKED]`, and `check_record_redacts`
in the self-test fails if it does not.

`assertions` comes out empty on purpose: the author adds the pass condition
before committing, and the two committed cases show the shape —
`recorded-0000-faux-echo` (a real offline faux run) and `recorded-0001-tool-turn`
(text + parallel tool calls in one assistant message, two stub results, real
usage). `crates/runtime/tests/behavior.rs` replays each twice and fails on any
divergence; `check_behavior.py` then locks the verdict (D76), `--update` in its
own commit.

Recording a **real provider** session is user-run and outside every gate: run a
normal session, point `record.py` at its file under
`~/.yi/sessions/<encoded-cwd>/`, read the diff before committing (redaction is a
pass, not a proof), then add the assertion.

## Environment

The harness's names, not the binary's. `check_env_surface.py` scans
`crates/*/src` for `YI_[A-Z_]+`, so an `EVAL_*` name never contains `YI_`.

| Name | Read by | Meaning |
|---|---|---|
| `EVAL_BINARY` | harbor adapter | local x86_64 musl build, uploaded into the container |
| `EVAL_BINARY_URL` | harbor, pier adapters | release URL to `curl`; pier's install spec is lowered into a Dockerfile and needs it |
| `EVAL_ROUTING` | `run.py`, harbor adapter | OpenRouter provider object, see [Task runner](#task-runner) |
| `EVAL_TIMEOUT_MULT` | harbor adapter, drivers | the trial's share of the task timeout (E12); rides the fingerprint as `+t<mult>` |
| `EVAL_SUITE_REV` | harbor adapter, drivers | the dataset digest in the fingerprint |
| `YI_LEVERS` | the binary, under `--eval` only | lever overrides (D220); declared in `scripts/guardrails/baselines/env_vars.json` |

Build the local musl binary with `just package-musl <version>` (target defaults
to `x86_64-unknown-linux-musl`). It cross-compiles via `zig cc`/`zig ar` and
fails closed, naming the exact fix, when either preflight is missing: `rustup
target list --installed` must contain the target, and `zig` must be on `PATH`. It
builds the `dist` profile, checks the ELF shape with `scripts/check_elf.py`
(64-bit, right `e_machine`, no `PT_INTERP`) and packages it with
`scripts/package.sh`. It does not run `scripts/smoke.sh`, since the cross binary
does not execute on the build host; the container's `yi --version` check is the
smoke test.

## Running a suite

```
export OPENROUTER_API_KEY=...           # passed through under its own name
export EVAL_BINARY=target/x86_64-unknown-linux-musl/dist/yi
PYTHONPATH=evals/adapters harbor run \
  --agent yi_harbor.agent:Yi \
  -d terminal-bench/terminal-bench@sha256:39d9f44b40420cde8fdcc087579c0d72a7e14fa3656d603c3f0d22fb35e27732 \
  -i terminal-bench/html-js-filter \
  --model openrouter/z-ai/glm-5.3-flash -k 1 -o runs/tbv4
```

`evals/drivers/tbv4_baseline.sh` is that command over a task subset with the caps
and the timeout multiplier (`evals/drivers/README.md`). `-d scale-ai/swe-atlas-qna`
is the same command with another dataset. pier is the same shape with
`pier run --agent yi_pier.agent:Yi`.

SWE-Atlas QnA grades `/logs/agent/answer.txt`. The adapter injects no instruction
text of its own; the one rewrite is the budget sentence (E12).

## Proxy egress (E2)

pier's air-gapped tasks reach the provider through an authenticated Squid
sidecar, and `ureq` does not read proxy env on its own (the `proxy-from-env`
feature is off). `yi` reads it instead, once at startup:

- `HTTPS_PROXY`, else `HTTP_PROXY` — lowercase accepted for both. Inline
  basic-auth is part of the url (`http://user:pass@squid:3128`); `ureq` parses
  and sends it.
- `NO_PROXY` — comma-separated hosts; `*` bypasses everything, an entry
  matches its own host and any subdomain. No ports, no CIDR.

Empty or unset means direct. A value that is not empty and not usable — a typo,
a scheme with no dialer, `socks5://` — fails startup with exit 2 and names the
value back as `scheme://***@host:port`, since the refusal goes to stderr and
inline credentials must not. It is never downgraded to a direct connection. A
faux run never dials a provider and skips the check.

## Budget discipline

Real-model rollouts are budgeted, deliberate, and ledgered.

- **Timeouts are never retried.** A failed run is a result, not a do-over.
- **Every real run appends a row to `docs/eval-ledger.md` before any claim
  cites it** — run-id, suite@rev, config fingerprint, metrics.
- **Defaults are what is benchmarked.** A high-effort variant is a separate
  ledgered row, never an edit to the scored config.
- μ metrics for a run come from the session-mining extractor over the trial's
  collected session directory. One extractor owns that schema; the adapters
  emit harness fields only.

## Fixtures

`fixtures/ask_events.jsonl` is a real `yi ask --json` transcript recorded
offline against the faux provider:

```
./target/debug/yi ask --model faux/faux-1 --json --session-dir <tmp> "Summarize the number two."
```

Faux reports zero usage, so the camelCase usage contract is pinned instead by
`fixtures/session/1787544431469_fixture-a.jsonl`, a copy of the committed v4
golden session (`crates/types/tests/fixtures/v4-golden.jsonl`) whose assistant
entry carries real non-zero `input`/`cacheRead`/`cacheWrite` and whose
compaction entry exercises `summarization_count`.

`fixtures/session-record/` feeds the recorder.

- `faux-echo.jsonl` is the session file from one real offline run, whose prompt
  carried both a needle and a planted fake credential:

  ```
  ./target/debug/yi ask --model faux/faux-1 --json --yolo --cwd <tmp> \
      --session-dir <tmp>/.yi-sessions \
      "record this cassette needle CASSETTE-NEEDLE-7 and stop
  api_key=sk-fake-3QpZr7Lm2Xv9Tb4Nc8Kd1Wq6"
  ```

  Every message entry is that run's own bytes. Its `ext_state` entry is omitted:
  it carries the recording machine's skills catalog and home paths. It is never
  regenerated — a re-run yields new ids and timestamps, so the self-test asserts
  semantic content, never fixture bytes.
- `tool-turn.jsonl` is hand-built beside the v4 golden (which is never edited),
  reusing its assistant shapes: thinking + text + two `bash` calls with the
  golden's exact argument key order (`zeta`, `alpha`, `nested`) and its real
  `input` 1200 / `totalTokens` 10750, then both tool results and a closing
  assistant. It pins the toolCall→`toolCalls` and toolResult→stub mapping.

## Live lane (D133)

```
python3 evals/run.py --live --binary target/debug/yi \
    --model openrouter/deepseek/deepseek-v4-flash-0731 --cap-usd 1 --out live-out
```

Scenarios live under `fixtures/live/<id>/` in the task shape (`task.json`,
`prompt.txt`, `repo/`, `reward.sh`); a `"kind": "refusal"` scenario names the
argv the binary must refuse and the stderr it must print; a `"kind": "cache"`
scenario asks one session its `turns` in order (`--continue` after the first)
and passes only when the warm turns read cached tokens. Every scenario ends
`pass`, `fail` or `inconclusive` — timeout, no key, provider trouble, budget —
and only `fail` is red. `--allow-faux` runs the lane's plumbing offline. The
run's HOME is fresh and has `telemetry.enabled`, so `run.json` carries
`yi stats telemetry` over every sidecar beside the statuses and `yi doctor`'s red
rows as `Invariant::<row>` classes. The CI job `live` posts
`scripts/live_report.py run.json` as one PR comment; without the
`OPENROUTER_API_KEY` secret it posts inconclusive and says why.

### Verdicts and history

`scripts/live_ledger.py baseline run.json` reads the last ten records from the
`telemetry` branch with the run's model and mode (its `+routing{…}` label): the
median and the worst run of ttft p50, the warm-turn cache hit rate of
`cache-warm` (its row's `warmRead` over `warmRead + warmInput`) and cost per
scenario, plus every error class seen. The whole-run hit rate is reported, not
ratcheted. The three workflows that run the suite pin one upstream through
`EVAL_ROUTING`, and `evals/selftest.py` holds them to one value.
`judge run.json baseline.json [again.json …]` names each band the first run
breaks and each ratchet every run lost. A ratchet's bound is the median's ratchet
or the worst run in the history, whichever is further, and the PR lane runs the
suite again, three runs at most, while `again baseline.json run.json …` says a
ratchet still holds, so one draw of an upstream never speaks for its median;
`live_report.py` prints the verdict at the top of the PR comment. Postmerge runs
the suite on main and `append`s the record to the branch, so a PR is judged
against main's last run.
