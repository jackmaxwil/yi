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
python3 evals/judge_replay.py all --dry --binary target/debug/yi --model faux/faux-1
python3 evals/judge_replay.py extract --corpus yi:$HOME/.yi/sessions \
    --corpus claude:$HOME/.claude/projects --out runs/replay
python3 evals/judge_replay.py label --out runs/replay --model <provider/model> --cap-usd 3 --jobs 4
python3 evals/judge_replay.py judge --out runs/replay --model <provider/model> --split fit \
    --limit 200 --cap-usd 3 --jobs 4
python3 evals/judge_replay.py match --out runs/replay --model <provider/model> --cap-usd 1
python3 evals/judge_replay.py report --out runs/replay
python3 evals/judge_replay.py mark --out runs/replay <boundary-id> objected
```

Stage 0 of `docs/plans/2026-09-24-seven-primitives.md`: does a judge that reads the owner's
words by address, and nothing after the boundary, predict the owner's first objection? A
boundary is a human message whose nearest message ancestor on the session tree is an assistant
turn end. `extract` (free) writes `boundaries.jsonl`: the intent record (file, entry id and text
of every earlier human message on that tree path), the turn (final text, one line per tool
call), the next message and the agent's reply to it. Sessions split 70/30 into `fit` and
`held-out` by the hash of the file name. `label` writes `labels.jsonl` (`objected`,
`check_revealed`, `accepted`), `mark` writes the owner's overrides to `marks.jsonl`, which win.
`judge` writes `verdicts/<model>-<prompt sha256>.jsonl`, `--limit` taking up to half positives,
and resolves every quote to a UTF-8 byte range of the named message. `match` asks, for each
positive the judge flagged, whether an objection names the owner's. `report` prints, per corpus
and split, n, positive rate, citation resolution, recall, specificity, balanced accuracy with a
session bootstrap 95% interval, catch rate and cost; the two no-judge baselines (0.5 by
construction); the gate line, PASS iff held-out resolution >= 0.95 and the interval's lower
bound > 0.5; and every judge run that has touched held-out, so tuning on it shows. Tune on `fit`.

What counts as the owner, surveyed on 2026-09-26 over 437 Yi files and 225 top-level Claude
Code transcripts:

- Yi: a `user` message, except in a child session (a `parentSessionId` header, or a file under
  `sub-*`: the corpus's `rlm-*/sub-*` children carry no header link) and except the runtime's
  own user-role notices: `[subagent …]`, `<ipython_state_restored>` and `[host] request`.
- Claude Code: 49,617 of 54,166 `user` entries are tool results. Skipped besides: `isMeta`
  (skill bodies, command caveats, image notes), `isCompactSummary`, an `origin` other than
  `human` (1,699 `<task-notification>` entries), slash commands (`<command-name>`,
  `<command-message>`: typed, but the text is the harness's template), `<local-command-stdout>`,
  `<create-pr-command>` (a button) and `[Request interrupted …]`. A leading `<system-reminder>`
  block (38 messages) is cut and the typed rest kept, its byte offset recorded. A prompt typed
  mid-turn is an `attachment` of type `queued_command` with `commandMode` `prompt` (163): the
  owner's words, in the intent record. Files under `subagents/` are skipped; `isSidechain`
  appears only there.

The corpus that day: 72 Yi boundaries in 35 sessions (17 held-out), 1,403 Claude Code
boundaries in 177 sessions (485 held-out).

Each call is `yi ask --json --confirm --here --schema replay/<phase>.schema.json` in an empty
temporary cwd under a temporary HOME (kernel prewarm off), its session under
`--out/sessions/<phase>`; `yi ask` runs in-process and never reaches the serve daemon, so no
`--solo`. With no terminal `--confirm` refuses every tool that is not read-only, but `read`,
`grep` and `glob` still run anywhere outside the credential stores, so a judge could read a
transcript past its boundary: the prompt forbids tools, each row counts the calls its model
made, and the report names the verdicts that ran one. Keys come from the environment only,
since the HOME is temporary; a refusal before any request (exit 2 or 4) stops the phase. Two
caps speak where they cut: `intent_chars=60000` drops the oldest messages, `digest_head=120`
shortens a tool-call head. The prompt rides argv, so an input over `argv_bytes=512000` is not
sent and its row says so. Cost is the provider's `usage.cost.total` (E14); a call in flight
when `--cap-usd` trips still lands, so a phase overshoots by at most `--jobs` - 1 calls.

A real run is the owner's: capped, and ledgered in `docs/eval-ledger.md` with the model, the
prompt hash and the gate line before any claim cites it. `--dry` is faux only, answers with
host-built JSON, and checks that no file under the corpus changed; it rides
`just postmerge-evals`. `fixtures/replay/yi/` is one real faux session driven over `yi acp`
(prompt, prompt, `_yi/rewind` to the second, prompt; its `ext_state` entry omitted and its cwd
replaced); `fixtures/replay/claude/` is three real transcripts and one subagent file with every
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
