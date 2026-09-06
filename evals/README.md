# evals/ — task-eval adapters

Pure Python, standard library only. No Cargo.toml, no workspace member, no
dependency of its own: harbor and pier are the host harness's installs and the
adapters import them; everything the self-test exercises is stdlib.

```
adapters/yi_usage.py       run command + usage parse + fingerprint (shared, pure)
adapters/yi_harbor/agent.py  harbor BaseInstalledAgent subclass
adapters/yi_pier/agent.py    pier BaseInstalledAgent subclass (+ install spec, allowlist, extras)
selftest.py                dry run: no docker, no API, no keys, no harness
run.py                     task runner over `yi ask --json`, scored by each task's reward.sh
record.py                  session JSONL -> behavior cassette (J3), redacted at record time
fixtures/                  a recorded faux transcript, a v4 session file, and the runner's tasks
```

Contracts: YI_DESIGN §15.2 (gates E1–E9), §15.4 (AA column → Yi source),
§15.5 (adapter shape), Appendix A.12 (the exact harness spans).

## Self-test

```
python3 evals/selftest.py
```

Exit code is the gate, and `just check` runs it: `check_guardrails.sh` invokes
it beside `check_behavior.py`, so a broken adapter fails the same lane Yi's
code answers to. It pins the run command's E6/E7/E8 contract, the camelCase
usage parse (E5), the all-or-none token rule (E9), corrupt-line tolerance,
pier's three extra columns, and the recorder's three: turn grouping with stub
FIFO and argument key order, the §10 redaction plant, and the refusals. Stdlib
only, no docker, no key — nothing about it needs the slow tier.

## Task runner

```
just postmerge-evals          # cargo build -p yi-cli, then the dry tier
python3 evals/run.py --dry --binary target/debug/yi --model faux/faux-1
```

A task is a directory under `fixtures/tasks/<id>/`: `task.json`
(`id`, `dryReward`, `timeoutSec`), `prompt.txt`, `repo/` copied verbatim into a
throwaway workspace, and `reward.sh` run there with the workspace as its cwd.
The workspace is `<tmp>/repo`, and the runner's own files sit beside it as
`<tmp>/events.jsonl` and `<tmp>/.yi-sessions` — never inside the graded tree,
where a `git diff` or clean-tree reward would score them as part of the
solution and the agent could read back its own transcript. The runner asks once
with `--json --yolo --cwd <ws> --session-dir <tmp>/.yi-sessions`, writes the
last assistant text to `<ws>/answer.txt` (SWE-Atlas QnA grades the answer file,
so a real run's answer must exist), and scores the task binary: `reward.sh`
exits 0 or the task scored nothing. A timeout is a result, never a retry.

`--dry` runs faux only — it refuses any other provider, because a gate spends no
API budget — and compares each reward to the task's `dryReward`, which is what
the exit code reports. Faux echoes the prompt and never calls a tool, so the dry
tier pins runner mechanics, not task solving: `answer-echo` scores 1 off the
echo (a runner that skips the answer file would score every real QnA rollout 0)
and `edit-file` scores 0 because the workspace is untouched (a runner that
scores an untouched workspace 1 flatters every real rollout). The third,
`clean-workspace`, is the only one a needle-grep reward cannot express: it lists
the graded tree and requires exactly the task's own files plus `answer.txt`. It
is a `just postmerge` sibling, not part of `just check`: it needs a built binary.

Drop `--dry` and the runner prints one JSON row per task plus a ready-to-paste
`docs/eval-ledger.md` row with its config fingerprint. Pasting it stays a human
act, and a real-model suite is user-run and budgeted (plan law 3).

## Axes (D140)

```
python3 evals/axes.py runs/tbv4 --suite tbv4@39d9f44b --model openrouter/z-ai/glm-5.3-flash
python3 evals/axes.py evals/fixtures/axes --json /tmp/rows.jsonl
```

One JSON row per v4 session file found under the directory, its context the
nearest ancestor holding a harbor `result.json` (reward, wall, timeout) or a
run.py `row.json`, else a journey; then the `docs/eval-ledger.md` row with
the `persistence`, `rigor` and `experience` triples on the right. Every column
is named with its source in `docs/plans/2026-09-06-tbv4-evals/axes.md`; the
signals come from `skills/yi/session-mining/extract.py` by import, the
telemetry columns from the `.telemetry.jsonl` sidecar beside each session.
Exit 2 when a trial is unmeasurable (a turn without usage, no assistant
message, no session at all). `evals/fixtures/axes/` holds one trial of each
shape and `expected.jsonl` pins the rows byte-for-byte (`check_axes`).

## ATIF (E10)

```
python3 evals/atif.py <session.jsonl> --agent-version $(yi --version | cut -d' ' -f2) > trajectory.json
```

The session file as an ATIF-v1.7 trajectory (harbor RFC 0001): one step per
user and assistant message on the main lane, the tool results that follow an
assistant message as its observation, metrics in harbor's convention (prompt
tokens include cache reads), final metrics summed and left unpriced when any
turn reported no usage. The harbor adapter writes `/logs/agent/trajectory.json`
after every run (`SUPPORTS_ATIF`), and `run.py --out` writes one beside
`events.jsonl`, so `harbor view` and the hub's judge can read a Yi trial.
`evals/fixtures/atif/tool-turn.trajectory.json` pins the conversion
(`check_atif`).

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
corrupt line means the session was read only in part. A silent skip would emit a
cassette that replays a session that never happened.

Redaction runs at record time, before the artifact exists: `record.py` imports
`redact` from `skills/yi/session-mining/extract.py`, so cassettes and mining
rows mask by one vocabulary (plan §10). `fixtures/session-record/faux-echo.jsonl`
carries a planted fake `api_key=` that must come out `[MASKED]`, and
`check_record_redacts` in the self-test fails if it ever does not.

`assertions` comes out empty on purpose: the author adds the pass condition
before committing, and the two committed cases show the shape —
`recorded-0000-faux-echo` (a real offline faux run) and `recorded-0001-tool-turn`
(text + parallel tool calls in one assistant message, two stub results, real
usage). `crates/runtime/tests/behavior.rs` replays each twice and fails on any
divergence; `check_behavior.py` then locks the verdict, `--update` in its own
commit.

Recording a **real provider** session is user-run and out of any gate's budget:
run a normal session, point `record.py` at its file under
`~/.yi/sessions/<encoded-cwd>/`, read the diff before committing (redaction is a
pass, not a proof), then add the assertion. Nothing in `just check` needs a key
or a network.

## Installing the binary

Both adapters install one static `x86_64-unknown-linux-musl` binary at
`/usr/local/bin/yi` — one step, so pier's 360 s agent-setup cap and harbor's
install phase both stay cheap (§15.3 lever 8).

These two names are the whole environment surface of `evals/`. They are the
harness's, not the binary's: no `YI_*` variable is involved, so
`baselines/env_vars.json` and its cap of 40 are untouched. Keep `YI_` out of
the names — `check_env_surface.py` matches the substring `YI_[A-Z_]+`, so an
`EVAL_YI_BINARY` would read as an undeclared Yi variable the day that scan
covers Python.

- `EVAL_BINARY_URL` — release URL to `curl`. The eventual default; no
  release lane serves one yet.
- `EVAL_BINARY` — local musl build, uploaded into the container. harbor
  only; pier's install spec is lowered into a Dockerfile and needs the URL.

Both paths verify with `yi --version` before the trial starts.

Build the local musl binary with `just package-musl <version>` (target
defaults to `x86_64-unknown-linux-musl`). It cross-compiles via `zig cc`/`zig
ar` — no `musl-gcc` or `cross` install required — and fails closed, naming
the exact fix, when either preflight is missing: `rustup target list
--installed` must already contain the target (`rustup target add
<target>`), and `zig` must be on `PATH` (or use musl-cross gcc /
cargo-zigbuild instead). It then builds the `dist` profile, checks the
result's ELF shape with `scripts/check_elf.py` (64-bit, right `e_machine`, no
`PT_INTERP` — the "static" claim above), and packages it with
`scripts/package.sh`. It cannot run `scripts/smoke.sh` — the cross binary
does not execute on the build host — so that step prints as skipped rather
than silently passing; the container's `yi --version` preflight above is the
real smoke test for this binary.

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

`evals/drivers/tbv4_baseline.sh` is that command over the six-task subset
with the caps and the one-hour multiplier (`evals/drivers/README.md`).
`-d scale-ai/swe-atlas-qna` is the same command with another dataset. pier is
the same shape with `pier run --agent yi_pier.agent:Yi`.

## Proxy egress (E2)

pier's air-gapped tasks reach the provider through an authenticated Squid
sidecar, and `ureq` does not read proxy env on its own (the `proxy-from-env`
feature is off). `yi` reads it instead, once at startup, in the binary:

- `HTTPS_PROXY`, else `HTTP_PROXY` — lowercase accepted for both, since
  container images set either. Inline basic-auth is part of the url
  (`http://user:pass@squid:3128`); `ureq` parses and sends it.
- `NO_PROXY` — comma-separated hosts; `*` bypasses everything, an entry
  matches its own host and any subdomain. No ports, no CIDR.

Empty or unset means direct. A value that is **not** empty and not usable —
a typo, a scheme with no dialer, `socks5://` — fails startup with exit 2 and
names the value back as `scheme://***@host:port`, since the refusal goes to
stderr and inline credentials must not. It is never downgraded to a direct
connection: in an air-gapped container a silently dropped proxy is an
unattributable hang, and the whole point of the sidecar is that nothing else
gets out.

These are the harness's own names, like `EVAL_BINARY` above: no `YI_*`
variable and no `baselines/env_vars.json` row.

SWE-Atlas QnA is graded **only** from `/logs/agent/answer.txt` inside
`<<FINAL_ANSWER>>` tags, and the verifier runs even after an agent timeout
(E4) — so drafting the answer file early and refining it in place is worth
real points. That is task and prompt discipline: the adapter never injects
instruction text of its own.

## Budget discipline

Real-model rollouts are budgeted, deliberate, and ledgered (plan law 3).

- **Timeouts are never retried.** A failed run is a result, not a do-over —
  pier's retry-exclude defaults already encode this, and the adapters do
  nothing clever about it.
- **Every real run appends a row to `docs/eval-ledger.md` before any claim
  cites it** — run-id, suite@rev, config fingerprint, §4 metrics. Claims cite
  ledger rows or they are vibes.
- **Defaults are what is benchmarked.** A high-effort variant is a separate
  ledgered row, never a quiet edit to the scored config.
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

  Every message entry is that run's own bytes. The one `ext_state` entry was
  dropped before committing: it carries the recording machine's skills catalog
  and its home paths, which are neither reproducible nor anyone else's business.
  It is never regenerated — a re-run yields new ids and timestamps, so the
  self-test asserts semantic content, never fixture bytes.
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
and only `fail` is red. The run's HOME is fresh and has `telemetry.enabled`, so
`run.json` carries `yi stats telemetry` over every sidecar beside the statuses
and `yi doctor`'s red rows as `Invariant::<row>` classes. The CI job `live`
posts `scripts/live_report.py run.json` as one PR comment; without the
`OPENROUTER_API_KEY` secret it posts inconclusive and says why.

### Verdicts and history

`scripts/live_ledger.py baseline` reads the last ten `run.json` records from the
`telemetry` branch — medians of ttft p50, cache hit rate and cost per scenario,
plus every error class seen. `judge run.json baseline.json` names each band the
run breaks and each ratchet it loses; `live_report.py` prints the verdict at the
top of the PR comment. Postmerge runs the suite on main and `append`s the record
to the branch, so a PR is always judged against what main did last.
