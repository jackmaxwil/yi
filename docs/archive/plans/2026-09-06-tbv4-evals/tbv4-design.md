# Terminal-Bench v4: what changed against TB2.1 and the harbor clone we hold

Read on 2026-09-06 against three clones under `ref/benchmarks/` (`$B`):

| clone | commit | date | what it is |
|---|---|---|---|
| `$B/terminal-bench` | `83c7a6172d629c6575b785ab12c8db787bb2e323` | 2026-09-02 | the v4 task repository, this session's shallow clone |
| `$B/terminal-bench-2-1` | as pinned in A.12 | 2026-08-21 | the TB2.1 dataset the drivers still name |
| `$B/harbor` | `39b85872597ea710077d8c93095059bca3ca4ed2`, `pyproject.toml:3` version `0.22.0` | 2026-08-21 | the harness; twelve days older than the task repo |

Line numbers below are valid for those clones. Only the A.12 spans of harbor
were opened; every `excise` path stayed closed, and no task body beyond
`bun-sourcemap-leak` (its `task.toml`, `instruction.md`, `tests/test.sh`,
`tests/Dockerfile`) was read.

## 1. The dataset moved, the tasks did not carry over

- `$B/terminal-bench/tasks/` holds 66 tasks (68 entries minus `README.md` and
  `dataset.toml`; `leaderboard/src/leaderboard/ci/static_analysis.py:45`
  pins `EXPECTED_TASK_COUNT = 66`). `tasks/dataset.toml:1-8` names the
  package `terminal-bench/terminal-bench` and every task by sha256 digest.
- `$B/terminal-bench/archive/` holds all 89 TB2.1 tasks by name
  (`comm -12` of the two listings is 89 lines). None of them is in `tasks/`:
  the intersection of `terminal-bench-2-1/tasks` and `terminal-bench/tasks` is
  `README.md` and `dataset.toml`. The six-task subset `tb21_baseline.sh:8`
  pins (`overfull-hbox fix-git regex-log db-wal-recovery password-recovery
  write-compressor`) exists only in the archive and in the TB2.1 hub package.
- The leaderboard is `4-0-0` on Harbor Hub
  (`leaderboard/leaderboard.yaml:1-14`, `dataset_version_refs: ['4']`), and
  the CI pins the dataset by digest: `core/hub.py:26-27`
  `DATASET = "terminal-bench/terminal-bench"`,
  `DATASET_REF = "sha256:39d9f44b40420cde8fdcc087579c0d72a7e14fa3656d603c3f0d22fb35e27732"`.
- Community submissions are closed: `leaderboard/SUBMIT.md:3-4` "Only
  submissions run by the maintainers will be added to the leaderboard at this
  time." A Yi row on the public board is not on the table; a Yi row on our
  ledger against the published rows is.
- Domain mix (`category` in every `task.toml`, taxonomy in
  `docs/TAXONOMY.md:1-16`, seven closed domains): Software 18, Science 14,
  ML 11, Operations 9, Hardware 5, Security 5, Media 4. TB2.1's
  `category = "software-engineering"` free string
  (`terminal-bench-2-1/tasks/cancel-async-tasks/task.toml:16`) became a
  closed set plus an open `subcategory`, enforced by
  `scripts/checks/check-task-fields.sh`.

## 2. Task format

`docs/task-template.toml` is the canonical shape; `tasks/bun-sourcemap-leak/task.toml` the worked example.

| field | v4 | TB2.1 | cite |
|---|---|---|---|
| `schema_version` | `"1.1"` in the task files; harbor's `TaskConfig` default is `"1.4"` and renames a legacy `version` key | `"1.1"` | `task.toml:2`; `harbor/models/task/config.py:796,824-826` |
| `artifacts = [...]` (top level, above the first section) | the only paths the verifier can read from the agent's container | absent (`artifacts = []`) | `task.toml:4-10`; `docs/task-template.toml:4`; `check-separate-verifier.sh:9` "nesting silently drops it" |
| `[verifier] environment_mode = "separate"` | required; verifier runs in its own container built from `tests/Dockerfile`, which must `COPY . /tests/` because "Separate-mode verifiers skip the tests/ upload" | shared container, tests uploaded at verify time | `task.toml:23`; `tests/Dockerfile:18-20`; `check-separate-verifier.sh:3-21`; harbor `verifier/verifier.py:96-98` |
| `[metadata]` | `category`, `subcategory`, `expert_time_estimate_hours` (0.75 to 60 across the set) | `difficulty`, `expert_time_estimate_min`, `junior_time_estimate_min` | `task-template.toml:6-14` vs `cancel-async-tasks/task.toml:13-20` |
| `[environment]` | `cpus` 1 to 16, `memory_mb`, `storage_mb`, `gpus` (three tasks at 1, H100), `network_mode = "public"`; 11 tasks add `environment/docker-compose.yaml` (multi-container, "agents cannot directly access backend services", `CONTRIBUTING.md` New Task Types) | `allow_internet = true` (now a deprecated alias) | `task-template.toml:19-25`; harbor `config.py:69-70,473,512` |
| `instruction.md` | ends with a canonical paragraph: "You have N seconds to complete this task. Do not cheat by using online solutions or hints specific to this task." where N equals `[agent].timeout_sec` | no suffix | `check-instruction-suffix.sh:3-9`; `bun-sourcemap-leak/instruction.md` last line |
| canary | `harbor-canary GUID 26b5c67b-…` on every file | same | `task.toml:1`, `tests/test.sh:2-3` |
| task and dataset `version` | new package-level field, distinct from `schema_version` | absent | harbor `CHANGELOG.md` "Task and dataset package versions"; `config.py:295-298` |

The agent sees exactly what TB2.1's agent saw: `/app`, an instruction, the
open internet. What it no longer sees is `/tests`, and what it no longer
matters that it writes is anything outside the declared `artifacts`.

## 3. Verifier contract

Unchanged in mechanism, hardened in isolation.

- `tests/test.sh:5-16`: `mkdir -p /logs/verifier`, run pytest with
  `--ctrf /logs/verifier/ctrf.json`, then `echo 1` or `echo 0` into
  `/logs/verifier/reward.txt` and exit with pytest's status. Binary reward,
  as TB2.1's `sqlite-db-truncate/tests/test.sh:25-29` was.
- harbor still reads `reward.json` before `reward.txt`
  (`verifier/verifier.py:227-234`), and the leaderboard defines success as
  `reward > 0` with errored trials counted as failures
  (`core/metrics.py:24-26`).
- The verifier runs in a separate container that receives only the declared
  artifacts (`tests/Dockerfile:22-24` pre-creates `/app` "Harbor uploads
  declared artifacts into the verifier and the parent dir must exist"). A file
  the agent pre-writes under `/tests` is never read, because `/tests` is baked
  into an image the agent never touches. YI_DESIGN E3's threat ("verifier
  trusts files the agent can pre-write") is closed by the task format rather
  than by the agent's permission profile.

## 4. Timeouts

| phase | v4 | TB2.1 | cite |
|---|---|---|---|
| agent | **28,800 s on all 66 tasks** (the 8 h cap `check-task-timeout.sh:7`); the template says "expect hard tasks with agents running for multiple hours" | median 900 s, max 12,000 | `task.toml:26`; `docs/task-template.toml:17-18` |
| verifier | 60 s to 18,000 s; 13 at 300, 13 at 600, 8 at 900, 4 at 1,800, 2 at 7,200, 1 at 18,000 | 900 typical | `[verifier] timeout_sec` over `tasks/*/task.toml` |
| build | 600 s default | same | `task-template.toml:20` |

A trial's wall time is now bounded by eight hours, not fifteen minutes. The
published Claude Code + Fable 5 row averaged 4,202.8 s per trial
(`leaderboard/submissions/2026-08-26-anthropic-claude-fable-5-max-claude-code.json`,
`avg_trial_duration_sec`). `tb21_baseline.sh`'s "everything ≥1200s is
excluded — wall clock, not USD, is the binding budget" (`evals/drivers/README.md`)
cannot select a v4 subset by timeout; every task has the same one. For a
non-leaderboard row, harbor exposes `--agent-timeout-multiplier`
(`harbor/cli/jobs.py:433`) and `--timeout-multiplier` (`:422`); the leaderboard
CI rejects any value but `None` or `1.0` (`static_analysis.py:115-124`).

## 5. The judge and the reward-hack column

TB2.1 published a `reward_hacks` metric (`terminal-bench-2-1/leaderboard/leaderboard.yaml`
metrics list: `reward_hacks`, `display_reward_hacks`, beside `accuracy_stderr`).
v4 removed the column: `grep -c reward_hack leaderboard/leaderboard.yaml` is 0,
and `metrics_schema` (`leaderboard.yaml:62-130`) is `accuracy`,
`accuracy_ci95_half_width`, `display_accuracy`, `total_tokens`,
`total_cost_usd`, `n_trials`, optional `successes`, `uncached_input_tokens`,
`cached_input_tokens`, `output_tokens`, `avg_trial_duration_sec`,
`pass_at_2..5`.

What replaced it:

- A submission carries `disqualified_trials`; the metric joins each one in as
  reward 0 (`core/metrics.py:77-90`: "the metric never mutates the hub —
  disqualified_trials is the record").
- Disqualification comes from `/judge` (`ci/judge.py:1-17`): every
  *successful* trial's ATIF trajectory is judged by a harbor task
  (`ci/analysis-task-template/`) whose instruction asks for three verdicts,
  `harness_level_cheating`, `task_level_reward_hacking`, `refusals`, each with
  reasoning citing step ids, and rule 6 "When uncertain, false"
  (`analysis-task-template/instruction.md`). Failed trials are not judged,
  "a trial that scored 0 can't inflate the leaderboard".
- Task-side hardening moved into review: `/cheat` adversarial trials and the
  `/fortify` hacker-fixer loop (`docs/REVIEWING.md`, steps 7-8), the 30-criterion
  implementation rubric (`docs/prompts/task-implementation.toml`), and
  `scripts/checks/test-tasks/fail-rubric-anti-cheat` and siblings.

For Yi this changes E3's shape: the risk is no longer a file the verifier
trusts; it is a judge reading the trajectory for hardcoded grader values,
fabricated answers, or verifier exploits. The judge reads ATIF, so an agent
without an ATIF trajectory cannot be judged at all (§7).

## 6. Leaderboard metrics and how they are computed

`leaderboard/src/leaderboard/core/metrics.py`:

- accuracy = successful trials / total trials, in percent (`:29-38`).
- standard error per task `p(1-p)/(k-1)`, summed over tasks, divided by
  `n²`, root, times 100 (`:40-49`); CI95 half width = 1.96 × that (`:21`).
- pass@k for k in 2..5 by the unbiased estimator `1 - C(n-c,k)/C(n,k)`
  averaged over tasks; a task with fewer than k trials is skipped for that k
  (`:52-74`).
- `MIN_TRIALS_PER_TASK = 5` (`static_analysis.py:46`); `TRIAL_CONCURRENCY = 24`
  (`:44`); every task must be covered; errored trials count as 0
  (`SUBMIT.md`, "Cover every task").
- Resource totals: `uncached_input_tokens`, `cached_input_tokens`,
  `output_tokens`, `total_tokens`, `total_cost_usd`, `avg_trial_duration_sec`
  from bulk trial metadata (`metrics.py:7-9,95-97`).

A reference row, for scale: Claude Code 2.1.231 + `anthropic/claude-fable-5`
at effort `max`: accuracy 44.55 ± 3.85, 330 trials, 147 successes, pass@5
0.6818, $7,265.01, 3.785 B tokens (3.727 B uncached input, 3.564 B cached,
58.6 M output), 4,202.8 s per trial
(`submissions/2026-08-26-anthropic-claude-fable-5-max-claude-code.json`).
That is $22 and 70 minutes per trial on a frontier model.

## 7. ATIF, the trajectory format

harbor 0.22.0 ships `models/trajectories/` (RFC 0001), `schema_version`
`ATIF-v1.0` through `ATIF-v1.7`, default v1.7 (`trajectory.py:15-26`).

- `Step` (`step.py:14-100`): `step_id` (1-based), `timestamp`, `source`
  (`system|user|agent`), `model_name`, `reasoning_effort`, `message`,
  `reasoning_content`, `tool_calls`, `observation`, `metrics`,
  `is_copied_context`, `llm_call_count`, `extra`.
- `Metrics` (`metrics.py:11-42`): `prompt_tokens` (including cached),
  `completion_tokens`, `cached_tokens`, `cost_usd`, optional token ids and
  logprobs, `extra`; `model_config = {"extra": "forbid"}`.
- `FinalMetrics` (`final_metrics.py:11-40`): `total_prompt_tokens`,
  `total_completion_tokens`, `total_cached_tokens`, `total_cost_usd`,
  `total_steps`, `extra`.
- An agent declares `SUPPORTS_ATIF` (`agents/base.py:50-52`); `claude_code.py:49`
  does, `pi.py` does not. Subagent transcripts join the root trajectory as
  `extra.is_sidechain` steps and count toward `final_metrics`
  (`CHANGELOG.md`, "Claude Code subagent transcripts included in trajectories").
- `--load-trajectory` seeds a session from an ATIF file when the agent
  declares `SUPPORTS_LOAD_ATIF_TRAJECTORY` (`installed/base.py:975-1001`).

Yi's v4 session JSONL carries everything a Step needs (assistant entries with
`usage`, tool calls with argument order, tool results, timestamps), so a
converter is a stdlib read of one file into one JSON, not a change to the
binary. It is optional for our ledger and required for `harbor view`, the hub
viewer, and any judge.

## 8. Telemetry fields and usage parsing

Unchanged where it matters, extended around the edges.

- `AgentContext` (`models/agent/context.py:8-31`): `n_input_tokens`
  "including cache", `n_cache_tokens`, `n_output_tokens`, `cost_usd`,
  `rollout_details`, `metadata`. Same four fields the adapters fill today.
- `TrialResult` (`models/trial/result.py:70-97`) adds
  `verifier_environment_mode`, `step_results` for multi-step tasks, and
  `compute_token_cost_totals` (`:99-138`) which sums per-step contexts and
  keeps "`n_input_tokens` is total input *including* cache". Timing phases
  `environment_setup`, `agent_setup`, `agent_execution`, `verifier` (`:93-96`)
  are the wall-clock columns.
- The Pi parse (`agents/installed/pi.py:230-263`) is byte-for-byte the E5
  contract: `message_end` → `usage.input`, `usage.output`, `usage.cacheRead`,
  `usage.cacheWrite`, `usage.cost.total`; `n_input = input + cacheRead`;
  `cost_usd = None` when the sum is 0. Its `grep -v '"type":"message_update"'`
  (`pi.py:224`) is still there, so E6 still names a live bug in the template.
- harbor does not price anything itself: cost is what the agent reports.
  `litellm` appears only in `agents/model_connection.py:132,176` (provider
  name resolution) and `telemetry.py`; there is no pricing table in
  `src/harbor`. Yi's self-reported `usage.cost.total` stays the only cost
  source, so the catalog cross-checks the ledger already does (rows 0001,
  0010-0013) remain the honesty check.
- Error classification (E1): `ERROR_PATTERNS` is now 28 rows at
  `installed/base.py:445-540` (A.12 cited 30 at `:445-521`), compiled at
  `:578-581`, classified at `:779-800`, and `set -o pipefail` is prepended at
  `:850`. New rows cover OpenRouter's "stream closed before completion" and
  "Response stalled mid-stream".
- `ModelConnectionSpec(passthrough=True)` (`agents/model_connection.py:138-152`)
  exists as the harbor adapter assumes.

## 9. Agent registration and the run CLI

- Out-of-tree registration is `--agent module:Class`; `--agent-import-path` is
  deprecated in favour of `--agent` (`cli/jobs.py:544-559,1480`).
- Task selection is `-i/--include-task-name` (`cli/jobs.py:1115-1116`). There
  is no `--task-name`; `tb21_baseline.sh:37` passes one.
- Attempts are `-k/--n-attempts` (`:410-411`); concurrency `-n/--n-concurrent`
  (`:490`); `--dry-run` exists (`:1250`); `--upload` (`:1233`) pushes trials to
  the hub, which the leaderboard CI reads.
- Output lands under `-o/--jobs-dir`, default `jobs`
  (`models/job/config.py:360`). `tb21_baseline.sh:14` reads `runs`, a
  directory harbor never writes.
- Datasets resolve from the hub as `org/name@ref` (`README.md`,
  `-d terminal-bench/terminal-bench@latest`); `harbor datasets download`
  materialises one locally (`cli/datasets.py:159`).

## 10. Gates E1-E9, re-read against v4

| gate | verdict | why |
|---|---|---|
| E1 exit-0 in `--json` | **holds** | `pipefail` at `base.py:850`, 28 patterns at `:445-540`; a non-zero exit is still a scored-0 trial |
| E2 proxy-aware transport | **not v4** | every v4 task is `network_mode = "public"` (`task-template.toml:23`); E2 stays a pier/DeepSWE gate |
| E3 deny rules on `/logs/verifier`, `/tests`, test files | **obsolete as stated, replaced** | separate verifier: `/tests` is baked into an image the agent never sees, the verifier reads only declared `artifacts`. What remains is the ATIF judge over the trajectory (§5). New E3': no hardcoded grader values, no fabricated answers, and a trajectory the judge can read |
| E4 `answer.txt` drafted early | **not v4** | no QnA task in v4; still the SWE-Atlas contract for the AA index |
| E5 camelCase usage | **holds** | `pi.py:250-256` unchanged |
| E6 no `grep` stage | **holds** | `pi.py:224` still carries the bug the gate warns about |
| E7 single-argv instruction | **holds** | `pi.py:180` `shlex.quote`; v4 instructions are longer (the canonical suffix, numbered requirements) |
| E8 `--yolo` | **holds** | every installed agent passes its bypass flag |
| E9 all-or-none telemetry | **holds, tighter** | `total_tokens` and `total_cost_usd` are *required* metrics (`leaderboard.yaml:64-72`); a trial with `None` cost cannot produce a submission row |

New gates the v4 contract adds:

| gate | contract | cite |
|---|---|---|
| E10 ATIF trajectory | `SUPPORTS_ATIF` and `trajectory.json` per trial, or the trial cannot be judged or viewed | `agents/base.py:50-52`; `ci/judge.py:1-17` |
| E11 five trials per task, multipliers at 1.0, every task covered | any claim shaped like the leaderboard's needs `MIN_TRIALS_PER_TASK = 5` and no timeout override; a subset or a multiplier is a ledger row, never a comparison | `static_analysis.py:45-46,115-124` |
| E12 the budget is in the instruction | the agent is told "You have 28800 seconds"; a runner that shortens the timeout without rewriting the instruction lies to the agent | `check-instruction-suffix.sh:3-9` |
| E13 pin by digest | `terminal-bench/terminal-bench@<sha256>` from `core/hub.py:27`, not `@latest`; the ledger's `suite@rev` cell is the digest's first twelve characters | `hub.py:24-27` |
| E14 self-reported cost is the only cost | harbor prices nothing; `usage.unknown` turns make a trial unmeasurable, which tb21_cost.py already refuses | `pi.py:263`; `evals/drivers/tb21_cost.py` |

## 11. What moved in harbor between 2026-08-21 and 2026-09-02

The harbor clone predates the task clone by twelve days; the `CHANGELOG.md`
head (all "Unreleased" above `2026-06-29`) shows the drift that matters:
`harbor leaderboard` deleted in favour of `harbor hub leaderboard`; hub auth
by personal API key (`harbor auth login`, `HARBOR_API_KEY`); job plugins
CLI-only; task and dataset `version` fields; subagent transcripts in ATIF;
egress-control probe fixed for non-Linux clients; `harbor check` and
`harbor analyze` as trials. None touches `BaseInstalledAgent.run`,
`AgentContext`, or the reward files. Re-verify A.12's line numbers after
`uv tool install harbor` lands a newer release than 0.22.0; the spans this
document cites are for the clone we hold.

## 12. Per-provider limitation: music-harmony through OpenRouter (2026-09-08)

The task requires Roman numerals in MusicXML `<function>` elements and its
verifier reads only that element. Through `openrouter/z-ai/glm-5.3-flash`
the model receives the tag as `[PROMPT_INJECTION]`: the persisted user
message carries the literal `<function>`, neither harbor's package nor this
tree holds that string, and every attempt on ledger rows 0018 and 0021
quoted it back (issue #280). The baseline's best attempt had three rule
violations and failed only on "no Roman numeral annotations found", so the
pass axis cannot move on this task whatever the harness does. The six-task
slice swaps it for `heat-pump-warranty` (local files, plain-text rules, a
CSV of decisions, no XML-like tag in the instruction); the config
fingerprint changes with the task list and the first row on the new slice
says so. music-harmony stays runnable by name through `TBV4_TASKS` for a
provider that passes the tag through.
