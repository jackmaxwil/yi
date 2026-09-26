# The second pass on the v4 failure paths: the kernel, the gates, the family

Landed: D160-D166 (0.190.0-0.199.0).

```
status:  planned 2026-09-08; stages S1-S11 land one PR each, in order.
tree:    0.189.0, last decision row D159. D160 onward below are drafts;
         claim against the header at land time.
issues:  one per stage under milestone "Evals on the ledger", opened
         before the stage's changelog row (D106).
evidence: docs/eval-ledger.md rows 0018-0023 and the 158 session files of
         the runs they name; the exploration notes of 2026-09-08 (this
         session), each fact cited to its file and line at c3aa01d.
lineage: docs/archive/plans/2026-09-08-failure-paths.md (the first pass),
         docs/archive/plans/2026-09-06-tbv4-evals.md (the instrument),
         docs/archive/plans/2026-09-06-prompt-surface.md (the prompt laws),
         docs/archive/plans/2026-08-28-native-methodology-and-triggered-skills.md
         (placement §15.5, §15.7), docs/YI_DESIGN.md §15.
```

## Context

Row 0023 (0.189.0, slice k=3) held pass at 3/18 while the runaways,
provider deaths and the todo tax were removed. Reading all sessions of the
last three runs against their tests leaves four loss shapes, each now a
product defect with the harness parts stripped away:

- a search run by hand in the reasoning channel until the 32k cap
  (photonic, every attempt);
- a stop one check short of a numbered requirement (cargo 24/27,
  foodstuff 10/13, bun 27/36, heat-pump 0.85 partial);
- a decisive tool output cut by the reducer and never re-read (heat-pump;
  77 pointers emitted, 0 read);
- a routing default that doubled cost without moving pass.

And one finding the user's question surfaced: the kernel was dead in every
trial, so `ipython`, `rlm` and every delegation pattern in the prefix were
unreachable. The user's item-by-item decisions after the docs review plus
the three follow-ups (kernel fixes, faster boot, the family transport and
status, readers as first-class children, more guidance) are the design.
This file grounds each in the tree at c3aa01d (0.189.0, last row D159).

The corpus is the 158 Yi session files of ledger rows 0018-0023 (42 on
the current harness: the three runs of 2026-09-08).

## Findings the corpus settles

| fact | number |
|---|---|
| thinking blocks persisted losslessly (`Content::Thinking`) | 694 blocks, 3.1 M chars, every session |
| tool calls, current harness | bash 361 · todo 270 · edit 113 · read 90 · write 44 · ipython 9 · grep 2 · plan 0 · `rlm.run` 0 |
| ipython results | 8 of 9 errored: "uv is required to set up the Python kernel"; one booted, then `ModuleNotFoundError: xlrd` |
| `orchestrate.md` attached (signal `prefilter`) | 158 of 158 sessions |
| bash results over 8 KiB after reduction / pointers emitted / read back | 3 / 77 / 0 |
| edit errors / `Edit rejected` | 28 / 6 (mismatch text already carries the tag and ±2 lines) |
| `done` evidence prose / with a command | 502 / 27; `set` with `[x]` closes an item with no evidence at all (211 `set` calls) |

**Why Yi never delegated.** Not a guidance gap first. `ensure_uv`
(`crates/kernel/src/bootstrap.rs:358-382`) needs `uv` or
`YI_INSTALL_UV=1`; the v4 images have neither, every `ipython` call
returned the install text, and the model used bash for the rest of the
hour. `rlm` lives only in the kernel. The protocol fragment with
`rlm.run`, worktrees and `h.result(schema=…)` was in the prefix of every
session (its one example names `TASK_SCHEMA`, defined nowhere). Doctrine's
own rule ("delegate what parallelizes … each big enough to justify a
child") also excludes a one-container, one-artifact task, so for v4 the
kernel matters as the place a search runs, not as a spawner. What the
prefix lacks once the kernel lives: the operating facts (kernels per
session and per child, what is shared, `%pip`, `context_keys`,
`kernel://`, `rlm.bash`, walls, depth, `rlm.wait` semantics), the
judgment rules (solo vs readers vs writers; bash vs kernel), the
reader-child pattern, a brief template, and worked examples.

## Laws (unchanged)

Deterministic triggers only; prompt bytes are paid per turn, growth is
one `--update` per stage in its own commit named in the row (D137);
issue before changelog row; one D-row per PR; ratchets in their own
commit; stdlib only under `evals/`; fixtures first; the scored config is
the shipped default; no assistant trailers; subject ≤72; never
reset/amend/rebase; scratchpad worktrees off `origin/main`; `pr open` /
`pr merge` via `scripts/forge_pr.py`; a stacked PR opens after its
predecessor merges. Placement (2026-08-28 plan §15.5, §15.7): a gate
beats prose; doctrine carries judgment; per-instance facts ride tool
results and descriptions; every API mention carries a runnable example.

## Stages, in order

| stage | what | row | crates |
|---|---|---|---|
| S0 | this plan lands as `docs/plans/2026-09-08-pass-levers.md` | changelog row, Refs #100 | docs |
| S1 | routing: drop `sort`, deprioritise slow upstreams | amends D159 | ai, types (doc), README |
| S2 | the kernel boots with what the machine has, fast, and says what it lacks | D160 | kernel, runtime, cli, evals adapter |
| S3 | reducer: 8 KiB floor, compress before cutting | D161 | tools |
| S4 | every edit refusal carries a usable tag and the lines | amends D117 | tools |
| S5 | artifact gate and closure gate, one override | D162 | types, runtime, cli |
| S6 | `done` evidence has a shape; `set` cannot close | amends D149 | runtime |
| S7 | spiral cutter on the reasoning stream; forced bash removed | D163 (amends D153) | ai, loop |
| S8 | the family's address space: objects and files across kernels | D164 | runtime (kernel, fetch, subagent), python rlm |
| S9 | the family's live view: status, discovery, depth | D165 | runtime (subagent, environment), python rlm, types |
| S10 | the working model in the prompt: doctrine, protocol, ipython, identity | D166 | runtime prompts, tools |
| S11 | paid rerun; ledger rows | measurement rows | evals, docs |

Versions 0.190.0 upward, claimed at land time. Each stage: forge issue
(milestone "Evals on the ledger"), row with `growth +N:` memo, `just
adr`, `just commit`, ratchets, `pr open`, `pr merge`. Every extractor
signal lands with a fixture entry in
`skills/yi/session-mining/fixtures/signals.jsonl` and a regenerated
`evals/fixtures/axes/expected.jsonl`. Order: S1 is one constant; S2 is
the defect behind the delegation finding and everything S8-S10 describes;
S3-S4 are tool-side; S5-S6 are the pass levers; S7 needs calibration; S8
and S9 must exist before S10 describes them; S11 measures.

---

### S1 · Routing (amends D159)

- `crates/ai/src/openai.rs:26`: `DEFAULT_ROUTING` becomes
  `{"preferred_min_throughput":{"p50":20},"preferred_max_latency":{"p50":10}}`;
  `routing_params` (:28-38) unchanged, config `routing` still verbatim,
  `{}` still load balancing.
- Test `crates/ai/tests/openrouter.rs:198` renamed
  `openrouter_requests_deprioritise_slow_upstreams_unless_the_config_says_otherwise`,
  asserts the object and the absence of `sort`.
- `crates/types/src/config.rs` `routing` doc comment and `README.md:223`
  name the new default.
- Row text: no `sort` keeps OpenRouter's price weighting and Auto Exacto
  (automatic on tool requests); the two thresholds deprioritise "rather
  than exclude"; Wafer at 5-10 tok/s falls behind Novita and Z.AI at
  catalog price; row 0023 paid $1.21 against $0.45 for the same passes.

### S2 · The kernel boots with what the machine has, fast, and says what it lacks (D160)

One decision, six parts, all in `crates/kernel/src/bootstrap.rs` unless
named.

**K1 · Toolchain ladder.** `ensure_uv` (:358-382) becomes
`find_toolchain(options) -> Result<Toolchain, String>`,
`enum Toolchain { Uv(PathBuf), System(PathBuf) }`:
1. `uv` on PATH or `~/.local/bin/uv` → `Uv`;
2. else a `python3` (also `python3.13/12/11`) whose
   `-c "import sys, venv, ensurepip; assert sys.version_info >= (3, 11)"`
   succeeds → `System`;
3. else `YI_INSTALL_UV=1` → install uv as today → `Uv`;
4. else `Err("no uv and no python3 3.11+ with venv on PATH. Install uv
   (curl -LsSf https://astral.sh/uv/install.sh | sh), or python3-venv, or
   set YI_INSTALL_UV=1 to let yi run that installer.")`.

**K2 · One fast build on either toolchain.** `bootstrap_venv` (:571-625):
- the interpreter that exists: `uv python install` only when step 2 found
  none; else `uv venv --python <found>`; no `--seed` on the uv path
  (`ensurepip` stays on the System path, it is the installer there);
- `--system-site-packages` on both, so the image's `xlrd`/`pandas` are
  importable and the venv's own ipykernel shadows by `sys.path` order;
- the required set in one call: `ipykernel dill <runtime_dir>` plus the
  four skill dirs (`PYTHON_SKILLS`), `--compile-bytecode` (uv skips
  `.pyc` by default, so the first `import pandas` paid the compile);
  failure fails the venv as today;
- the extras lazily in one more call: only what
  `missing_extra_imports(venv python)` (:322) still cannot import, with
  the degrade-to-warning shape the skills loop already has (:600-620);
- `kernel_ready` (:565) runs one probe subprocess
  (`import ipykernel; <RUNTIME_READY_CHECK>`) instead of two;
- `BootstrapVersion` gains `toolchain` with serde default `"uv"`; an
  existing venv is never rebuilt by this change.
Not levers: sharing `~/.cache/uv` across trials (fresh container each),
`uv cache prune --ci`. Cache and venv both default under `$HOME`, so
uv's hardlinks apply (astral docs: same filesystem or it copies). The
IPython tips page the user linked carries nothing on startup; the
kernel-side levers are uv's and ipykernel's.

**K3 · A missing module names its install.** `crates/runtime/src/kernel.rs`
`execute_user_cell` (:532): `error.ename == "ModuleNotFoundError"` →
append "`x` is not installed in the kernel. Run `%pip install x` in a
cell; `pip` in bash installs into a different Python." (name parsed from
`evalue`).

**K4 · The environment and the doctor say what the kernel is.**
`crates/runtime/src/environment.rs` gains a per-request line
`kernel: ready (python3 3.12, rlm)` / `kernel: booting` /
`kernel: unavailable (<K1 step-4 text>)` from `KernelService` state (a
runtime fact, no prefix bytes). `crates/cli/src/doctor.rs:47` gains
`kernel-toolchain` (`ok("uv <path>")`, `ok("python3 3.12 <path> (no uv)")`,
or the step-4 text) and `kernel-boot` (venv build ms or `cached`, kernel
start ms, bootstrap-cell ms, `link-mode: copy` when uv reports it);
`yi doctor --fix` builds the venv (K1+K2) when the toolchain row is green
and the venv is not ready. The doctor row-list pin test gains both rows.

**K5 · The adapter fails loud, off the clock.** `evals/adapters/yi_harbor/agent.py`
`install()` (:24-43): after the version check, `yi doctor --fix`; raise
when `kernel-toolchain` is red. The venv is then built inside the 360 s
agent-setup cap (`evals/adapters/yi_pier/agent.py:50`) before the hour,
and prewarm finds it ready. No `YI_INSTALL_UV`: every v4 image carries
python3 for its own pytest, the shipped ladder covers them, and the
scored config stays the shipped default. `evals/selftest.py`
`check_command` asserts the doctor step. Verify at implementation that
harbor charges install to its own phase; if not, the build stays in
prewarm and the row says so.

**K6 · Proof.** `crates/kernel/tests/kernel_e2e.rs` (temp HOME per `home()`
:33; `YI_KERNEL_VENV` on a temp dir): with PATH reduced to a dir holding
only a `python3` symlink, `ensure_kernel_python` builds through System,
`has_runtime` passes (the ready check imports `rlm` and asserts
`rlm.run`), and a `KernelManager` cell prints `callable(rlm.run)`.
Network for pip, as the uv e2e needs it; about 30 s.

**Extractor.** `kernel_dead` (an ipython result starting "uv is required"
or "no uv and no python3"), `module_missing` (the K3 line). Row 0023
scores `kernel_dead` ≥1 on most tasks; the rerun must score 0.

The ipython schema sentence at `crates/tools/src/ipython.rs:53` ("Yi's own
virtualenv, not the target project's") becomes "Yi's own venv over the
machine's site-packages: what `python3` here imports, a cell imports; a
project's own venv is reached through a `%%bash` cell that runs its
interpreter or runner." Prompt bytes otherwise: none.

### S3 · Reducer: 8 KiB floor, compress the middle before omitting it (D161)

`crates/tools/src/reduce.rs`, plus the bash description string.

- `REDUCE_FLOOR = 8_192` (:17); the description at
  `crates/tools/src/builtins.rs:403` says "over 8,192 bytes"; test
  `the_bash_description_names_the_reducer_floor` formats the constant
  into the expected substring (2,048 is written twice today with nothing
  tying them).
- `fn compress(text) -> String` after `strip_ansi` (:41), before the
  per-program dispatch: keep only the text after the last `\r` per line;
  trim trailing whitespace, collapse blank-line runs to one;
  `collapse_repeats` (:137) stays; drop a later verbatim duplicate of a
  line of 8+ chars and suffix the first ` [×N]`; keep only the last
  instance of a progress line (`\d{1,3}%`, `[=>`, `━`, `#####`). Char
  scans, no regex crate; `never_worse` (:79) still guards.
  `// ponytail: global dedupe reorders nothing but hides a repeated table
  row's position; per-block dedupe if a rollout shows it mattered`.
- `cap_lines` (:159) takes a byte budget beside the line cap: head 2/3,
  tail 1/3 of `REDUCE_FLOOR` bytes, so a 700-line dump at 40 chars shows
  about 200 lines, not 120; `max_output_lines` still overrides; marker
  and pointer shapes unchanged.
- Tests in `crates/tools/tests/tools.rs` beside `:422`:
  `a_result_under_eight_kib_is_never_reduced`,
  `progress_and_duplicate_lines_compress_before_the_middle_is_cut`.
- Signals: `reduced_results` (count of `[N lines omitted`);
  `pointer_never_read` (extract.py:378) is the mover.

### S4 · Every edit refusal carries a usable tag and the lines (amends D117)

The mismatch arms already do (`mismatch.rs:66-84`, `messages.rs:8-45`).
Remaining refusals in `crates/tools/src/hashline/messages.rs`:
`missing_snapshot_tag_message` (:332) mints the current tag through
`snapshots.record` as `mismatch_error` does (`patcher.rs:612-622`) and
appends `[path#TAG]` plus the lines the edit named, so the retry needs no
read; `unseen_lines_message` (:400-437) adds the minted tag. Tests beside
`hashline_patcher.rs:243`:
`a_missing_tag_refusal_carries_a_usable_tag_and_the_lines`,
`a_rejected_edit_classifies_as_stale_tag` (pins `ToolErrorKind::StaleTag`
to `EDIT_REJECTED_PREFIX`, `hashline/tool.rs:795-802`, untested today).
Small by the corpus (28 edit errors in 158 sessions); lands because it is
two functions and the test is the missing pin.

### S5 · Two soft gates at a clean stop, one override (D162)

Both in `crates/runtime/src/todo/coupling.rs` on `Cycle` and the
`intercept_stop` closure (:800-860), after `claim_redrive`, before
`stop_posture`; a Length or Error stop never reaches them
(`is_terminal`). Artifact before closure: nothing to check without the
file.

**Plumbing.** `Options` (:418) gains `cwd: PathBuf` and `gates: Gates`
(`wiring.rs:335-352` passes `wiring.cwd`, `wiring.gates`).
`crates/types/src/config.rs`: `Gates { artifact: bool, closure: bool }`
(Copy, default true) on `RuntimeWiring`; `UserConfig.gates:
Option<GatesConfig>` with `artifact: Option<bool>`, `closure:
Option<bool>` (the `mcp.enabled` shape, :115-118). `--no-gates` on `yi
ask` (`main.rs:119`) sets both false and wins over config.
`crates/runtime/tests/kernel_across_sessions.rs:91` gains the field.

**Artifact gate.** `on_prompt` (:740) parses candidate paths from the
prompt: tokens with `/` or a data or code extension (`.py .js .ts .json
.csv .txt .md .yaml .yml .xml .html .sh .rs .go .c .cpp .java`) after a
product word (`write create save produce output to at in called named`)
or inside backticks, relative to `cwd`, deduped, capped at 8. **A
candidate that exists at prompt time is an input and is dropped**; the
rest are `Cycle.artifacts`. `Cycle.tool_turns` counts turns carrying a
tool call. At `tool_turns == 3` with none on disk, one steer
(`Cycle.artifact_steered`): "None of `<paths>` exists yet. Write a first
version now, even a stub that runs, and improve it in place." At a clean
stop with one still missing, the first stop is refused once
(`artifact_missing`, rung 0): "`<path>` does not exist. Write it, then
stop."; the second passes and records `artifact_waived`.

**Closure gate.** A checker is resolved once in `on_prompt` from `cwd`
(top level and one level down): `test*.py`, `*_test.py`, `tests/`,
`check*.py|sh`, `verify*.py|sh`, `run_tests*`, `Makefile` with a `test:`
or `check:` target, `pytest.ini`, `pyproject.toml` with `[tool.pytest`,
`package.json` with `"test":`, `Cargo.toml`; stored as the command
(`python3 <file>`, `pytest`, `sh <file>`, `make test`, `npm test`, `cargo
test`). `on_turn` bumps `last_write` on a landed edit/write/mutating
bash and `last_check` on a bash command naming the checker's file or
command. At a clean stop with a checker and `last_write > last_check`
(or never checked), the first stop is refused once (`closure_unrun`):
"Run `<checker>` and quote its result before stopping."; the second
passes and records `closure_waived`. Never on a turn that called
`ask_user` or while children run (the `stop_posture` guard).

**Tests** in `crates/runtime/tests/todo_coupling.rs`:
`artifact_paths_are_read_from_the_prompt_and_inputs_are_not`
(heat-pump's "save to `/app/output.json`", photonic's "router at
/app/route.py", an existing `/app/data/x.csv` excluded);
`the_artifact_steer_fires_once_on_the_third_tool_turn_with_nothing_on_disk`;
`a_clean_stop_with_a_missing_artifact_is_refused_once_then_waived`;
`a_checker_that_did_not_run_since_the_last_write_refuses_the_first_stop`;
`no_gates_disables_both`. Signals: `artifact_steer`, `artifact_refused`,
`closure_refused`, `gate_waived`.

### S6 · `done` evidence has a shape, and `set` cannot close (amends D149)

`crates/runtime/src/todo/mod.rs`:
- `Op::Done` (:605-620): after the blank check, evidence must contain a
  backtick-quoted command with a non-blank body and at least twelve
  characters outside the backticks; else `TodoError::EvidenceShape {
  label }`: "done needs evidence shaped `<command>` then the output line
  it produced, e.g. `pytest -q` 3 passed in 0.41s; prose is not evidence
  for {label:?}". Schema string `tool.rs:30`: "done: the command in
  backticks and the output line that proves it".
- `Op::Set` (:550): a checklist may not move an item to `[x]` that was
  not already done; refused with `TodoError::SetClosed { labels }`:
  "`set` cannot close {labels}; `done <label>` with evidence closes an
  item" (`text::merge` reports the newly-closed labels). An item already
  done stays done through a `set`.
- Tests in `crates/runtime/tests/todo_e2e.rs`: prose refused,
  `` `python3 check.py` all 12 checks passed `` accepted, `Target::Phase`
  unchanged, a `set` with a new `[x]` refused and one re-sending an
  existing `[x]` accepted. Signal `evidence_shape_refused` (both texts).

### S7 · Spiral cutter on the reasoning stream; the forced tool goes (D163, amends D153)

**Detector** `crates/loop/src/spiral.rs` (new, about 250 lines, no deps),
a port of omp's `ThinkingLoopDetector`
(`ref/agents/omp/packages/ai/src/utils/thinking-loop.ts:52-306, 495-564`):
exact tail cycle (Z-array over the reversed 4096-char tail, unit ≤1024, 4
repeats ≥180 chars short or 3 repeats ≥1024 long, unit must hold a
letter); segments on a blank line or at 700 chars, normalized
(lowercase, backticks unwrapped, non-alphanumerics to spaces, tokens
without a letter dropped, headings and bold titles stripped), under 60
chars ignored; trigram-Jaccard ≥0.8 against the last 16 segments, cluster
≥4 after 8; lexical novelty ≤0.2 against the last 8 segments' vocabulary
for 8 consecutive segments, the run reset by a concrete anchor (backtick
span, `word.ext`, `a/b/c`, `snake_case`, `camelCase`) unseen in the last
8. `push(delta) -> Option<Trip { detail, chars }>`, `reset()` per turn,
disarmed by a text or tool-call start. No model-family gate: the trigger
is the text.

**Calibration before the constants ship**: `a scratch `spiral_calibrate.py``
(stdlib replay of the same rules) over every thinking block in the 158
sessions in 64-char chunks, labelled "ended `length` with no tool call"
(spiral) or "ended with a tool call" (healthy). Ship only if every
healthy block is clean and the photonic length turns trip before 16k
chars; else raise `SEGMENT_MIN_CLUSTER` or `LEX_STALL_MIN_RUN` by one and
record the numbers in the ADR.

**Abort plumbing** (today `provider.rs:153-158` binds `_signal` and
`request.rs:190-222` `pump_sse` reads to EOF): `InterruptSignal::cut()`
beside `fire()` (`crates/loop/src/interrupt.rs`), a second `AtomicBool`;
`spawn_provider_stream` (`request.rs:330-336`) threads it into
`pump_sse`, checked before each read; set → return `Ok(())`, the dropped
reader closes the connection. `ProviderStream::stream_raw` passes it.
Test in `crates/ai/tests`: a 100-chunk fake body with the flag set after
10 delivers ≤11 events.

**Loop** `crates/loop/src/run.rs`: in `stream_assistant_response`
(:472-482) feed every `ThinkingDelta` to the detector; on a trip:
`signal.cut()`, stop consuming, drop the thinking block from the partial
message (a runaway must not ride every later request as cached input),
`stop_reason = Length`, `usage.reasoning = Some(chars / 4)` (no usage
chunk arrives on a cut), `error_message = None` so the stream retry never
sees it. The ladder (:733-745): `length_force` and `LENGTH_FORCE_TEXT`
deleted with their test; `LENGTH_REDRIVE_TEXT` becomes "The reply hit the
output limit before any tool call. Pick the most boring viable option and
act on it now: make the tool call, then explain."; details `{rung, cut,
detail, reasoningChars}`; `LENGTH_STOP_AT = 3` stays consecutive. Tests in
`loop_events.rs` beside :454: a six-times-repeated 200-char paragraph
yields one Assistant with `stopReason: length`, no thinking block, `cut:
true`, one redrive; distinct paragraphs untouched; the interrupt test at
:931 still passes. Signal `spiral_cut`; `length_forced` stays as a zero
tripwire.

### S8 · The family's address space: objects and files across kernels (D164)

Verified today: `context_keys` at spawn (8 variables, 4 KiB each, JSON,
one way); `kernel://<agent>/<var>` returns a `repr` cut at 8 KiB
(`kernel.rs:634, 694-722`), any direction, since the kernel map is
family-wide; `local://` reaches the workspace and the spill dir only
(`schemes.rs:57-80`), so a worktree child's files are unreachable until
`merge_worktree`. Decision: "a family shares one address space for
objects, files and transcripts; the filesystem is the transport".

- **Objects.** The family dir `<root session_dir>/family/` is the
  transport for every kernel-to-kernel object. `rlm.fetch("kernel://<agent>/<var>")`
  between family members returns the live object: the owner's kernel
  dills the variable to `family/<agent>.<var>.dill` (the snapshot code
  path and its 16 MiB per-variable cap, `snapshot.rs`), the host answers
  with the path, the reader undills. Text stays for a non-family reader,
  for what dill refuses, and under `as_text=True`; `VARIABLE_MAX_CHARS`
  applies to text only. `rlm.put(name, obj)` / `rlm.get(name)` /
  `rlm.ls()` are the blackboard on the same dir with a JSON sidecar
  `{owner, at, bytes, type}`, addressable as `family://<name>`; a child's
  `put` is how a large result comes home without a transcript byte.
  `context_keys` stays for the brief.
- **Files.** `tree://<agent>/<path>` reads a file from a family member's
  cwd (its worktree), read-only, walled by the reader's `deny_read`; a
  parent inspects a writer's change before `merge_worktree`.
- **Tests.** `crates/runtime/tests/subagent_e2e.rs` (existing rig): a
  1 MiB dict round trip parent→child→parent through `kernel://` and
  `family://`; `as_text` fallback; `tree://` on a worktree child and its
  refusal under `deny_read`. `python/yi_runtime/tests`: `put/get/ls`
  signatures.

### S9 · The family's live view: status, discovery, depth (D165)

Verified today: `list_subagents()` gives `running|completed|error`;
`rlm.list_agents()` names the family; `history://<agent>` renders a whole
transcript or one entry (`schemes.rs:149-167`); the environment shows
`children: N running (names)` and nothing else (`environment.rs:226-243`);
`ChildUpdate` events (tools, tokens, activity, a 240-char preview) reach
the TUI, never the model; a child's `ask_user` terminates its turn with
the question as its last text (`auto_review.rs:212-225`), so the parent's
notice reads "finished" with a question as the answer. Nothing computes
"stuck". Decision: "a parent reads its children's state as a fact of the
environment, and every member can read any member's tail".

- **`rlm.status(name=None)`** → for each member the caller may see
  (children; for a child, parent and siblings): `{name, state, turns,
  tokens, cost, last_tool, idle_s, worktree, note}`, `state` in
  `running | finished | failed | needs_you | stuck`. Read from the
  member's session store, the records the loop and coupling already
  write: `needs_you` when the last assistant message ended in an
  `ask_user` call or a todo is blocked `on user`; `stuck` when the last
  three entries carry a `repeat_break`, a `length_redrive` at rung ≥2, a
  `todo_intercept` reason `let go`, or no `MessageEnd` for 300 s while
  running (`note` names which). Deterministic; no model rates a child.
- **`history://<agent>?tail=N`** renders the last N entries compact (tool
  name and arguments cut at 200 chars, first line of each result and
  text); `?since=<seq>` reads forward. `rlm.list_agents()` for a child
  carries sibling names and states.
- **Environment line**: `children: 2 running (a, b) · 1 finished (c) · 1
  needs you (d: asked a question) · 1 stuck (e: repeat_break)`, present
  whenever the session has spawned; a `needs_you` child also rides one
  steer message: `[child d asks: <question>] answer with rlm.send("d",
  "…", followup=True)`.
- **Depth.** `rlm.maxDepth` in config (default 1, ceiling 3) replaces
  `DEFAULT_MAX_DEPTH`; `DEFAULT_MAX_CHILDREN` 8 per parent stays; a
  family-wide cap of 16 live sessions so a depth-2 fan-out cannot
  multiply.
- **Tests.** `status()` states pinned by writing the records into a
  child's store; the environment line for each state; `tail=`/`since=`;
  a spawn refused at the family cap; the steer message on `needs_you`.

### S10 · The working model in the prompt (D166)

The user's ask: Yi learns in context how to use all of this. Placement
per the laws: judgment and the reader/writer rules in doctrine (paid
every turn, so compact); the procedure and the worked patterns in
`orchestrate.md` (attached only on the decomposition signals, so its
size is paid where it applies); per-instance facts in the ipython
description; the capability lines in identity. No skill: `skills/yi` is
not in the binary, so a skill is invisible to every trial. Every example
runs and is awaited (`check_prompt_examples.py` reads `async def` names,
so the synchronous `rlm.bash` passes). Every fact stated is one S2, S8
or S9 made true, or one verified in the tree this session (the kwarg
set `name model thinking fork isolation deny_write deny_read deny_url
context check`, `subagent.rs:262-290`; `effort` is `thinking`; no `cwd`
or `timeout` kwarg; `rlm.wait` drains what it reports; `h.result(schema,
timeout=900)` returns `{text, json?}` and raises on mismatch; a `check=`
child owes `{"value","discoveries"}` and its result is withheld while
red; a child's report is its last assistant text; `write` overwrites
unread; a capped `read` says `continue with offset=N`).

**doctrine.md** — the Delegation section (:308-322) becomes "Working
model" (about 3.4 KB for 0.9 KB), two kinds of child named apart:
**readers** (explore, recon, review, compare, read references; common,
cheap, walled) and **writers** (execute a todo with a check; rare,
owned). Six rules with exits:

1. Solo, readers, or writers. Solo: one file, one checker, or a chain
   where each step needs the last. Readers: when the questions
   outnumber the turns you can spend reading (map a repository by area,
   test three or more candidate causes at once, read a corpus or a
   vendor tree, compare N implementations, review your finished work
   cold). Writers: three or more independent units (no shared file, no
   edge), each a session's worth, each with a check written first. Never
   delegate the reasoning the answer turns on: a reader brings evidence,
   you conclude.
2. A reader is walled and cheap: `deny_write=["."]` (every edit, write
   and cwd-naming bash refused, reads not), in your tree, no worktree,
   a cheaper model when `rlm.find_models` offers one, one question, the
   places to look, findings as `{path, line, claim, evidence}` with the
   quoted line. A reader's claim is data: open the cited line before you
   build on it; a claim with no citation is dropped at the schema seam.
3. Bash or the kernel. bash runs one command whose output you read once
   (build, test, git, scripts). The kernel runs anything with state: a
   loop over results, a number, a table, a search, an API probe, a dump
   parse, the aggregation of children. A search run in prose is a
   program not yet written. `%%bash` in a cell when the command needs the
   kernel's variables; `h = rlm.bash("cargo build")` to overlap.
4. The flow. The todo list is yours; the plan is the hand-off. Lift a
   todo into the plan only when a child executes it, with its check; the
   child's report is data; you run the check; you step your todo,
   blocked `on child` while it runs.
5. Ownership and waiting. Readers own nothing and share your tree. Two
   writers never own one file: `isolation='worktree'` each,
   `merge_worktree` in dependency order, or a `deny_write` list that is
   the complement of the scope. Keep working what you kept; `await
   rlm.wait(120)` only when the next step needs a result, and read the
   names it returns because they are gone next call; `rlm.status()`
   between waits: `needs_you` gets `send(..., followup=True)`, `stuck`
   gets `history://<name>?tail=20`, an interrupt and a corrected respawn.
   Collect with `await h.result(schema=SCHEMA, timeout=900)`; reap with
   `rlm.delete_subagent`. Depth is one unless config raises it.
6. Data stays in kernels. Large results come home by `rlm.put`/`get`,
   files in a worktree by `tree://<name>/<path>`, live values by
   `await rlm.fetch("kernel://<name>/<var>")`. The transcript carries the
   digest and the decision, never the data.

Plus four lines in "Tools and output" (:275-294): read one large window
and edit from its anchors; a capped read names the offset to continue;
`read` before `write` on a path you did not create; `grep def=true` or
`block=true` before reading a large file for one function. Method step 4
(:120-124) gains "readers, when the questions outnumber the turns".

**orchestrate.md** (3.8 KB → about 11 KB; attached on `prefilter`,
`edit_before_read`, `files_matched > 5`, `failed_check_after_edit`,
`tool_calls_per_turn > 4`): replaces the undefined `TASK_SCHEMA` with two
runnable cells and adds a Patterns section for in-context learning.

Reader fan-out:

    FINDINGS = {"type": "object", "required": ["findings"],
                "properties": {"findings": {"type": "array", "items": {
                    "type": "object", "required": ["path", "line", "claim", "evidence"],
                    "properties": {"path": {"type": "string"}, "line": {"type": "integer"},
                                   "claim": {"type": "string"}, "evidence": {"type": "string"}}}}}}
    areas = {"auth": "crates/auth/**", "storage": "crates/store/**", "cli": "crates/cli/**"}
    readers, findings = {}, {}
    for name, scope in areas.items():
        readers[name] = await rlm.run(
            f"Read {scope} only. Question: where is a session token minted, stored and checked? "
            f"Report JSON matching FINDINGS: one finding per site, `evidence` is the quoted line.",
            name=f"read-{name}", deny_write=["."])
    while readers:
        for name in (await rlm.wait(120))["updated"]:
            r = await readers.pop(name).result(schema=FINDINGS)
            findings[name] = r["json"]["findings"]
            await rlm.delete_subagent(name)
    # open every cited path:line yourself before building on it

Writer with ownership:

    SCHEMA = {"type": "object", "required": ["outcome", "files", "check"],
              "properties": {"outcome": {"type": "string"},
                             "files": {"type": "array", "items": {"type": "string"}},
                             "check": {"type": "string"}}}
    brief = """Port crates/foo to the new API.
    Acceptance: `cargo test -p foo` exits 0.
    Scope: crates/foo/** only; do not touch crates/bar.
    Report as JSON: {"outcome": one line, "files": changed paths, "check": the command you ran and its last line}."""
    h = await rlm.run(brief, name="foo", isolation="worktree", context_keys=["api_notes"])
    # keep working your own todos here
    moved = await rlm.wait(120)
    r = await h.result(schema=SCHEMA, timeout=900)
    await rlm.merge_worktree("foo")
    await rlm.delete_subagent("foo")

Beside them: the two brief templates (reader: one question, where to
look, what not to conclude, the findings shape; writer: title,
acceptance, check verbatim, files in and out of scope, binding
constraints, JSON report shape, "blockers as facts, not questions"); the
fan-out shape (spawn all, one `rlm.wait` loop, eight at a time, reap as
they land); the cold reviewer as a reader whose brief is only the
acceptance list and the checks; the failure branch (`failed:` or `stuck`
→ `history://x?tail=20`, fix the brief, respawn; never repair inside a
child's tree by hand); context slicing (`context_keys` for the brief,
`kernel://main/<var>` for what is live, `family://` for what was put,
`tree://` for worktree files, `fork=N` only to continue a thread); the
caps (depth one by default, eight per parent, sixteen per family).

Patterns, each two or three lines with its one code line:

1. Map-reduce: readers per shard `put` their findings; the parent
   reduces with pandas in its kernel.
2. Hypothesis tournament: N children with `check=`; `result()` is
   withheld while red; take the first green, `interrupt` the rest.
3. Best-of-N: N writers in worktrees on one todo at different `thinking`
   levels or models; run the check in each through `tree://`; merge the
   winner, discard the rest.
4. Persistent specialist: one child kept all session as the test runner
   or the reference reader; `rlm.send(name, q, followup=True)` reuses its
   warmed context.
5. Live pair review: write into a kernel variable; a reader fetches
   `kernel://main/draft` on each `send` and answers with findings.
6. What-if fork: `fork=N` into a worktree for the risky refactor while
   you continue the safe one; discard on red.
7. Watchdog reader: a child polls `history://main?tail=20` every 60 s and
   sends one line when your tail repeats a tool batch three times.
8. Swarm with a blackboard: siblings `put` under their names and `get`
   each other's before starting a shard; `status()` says who still runs.
9. Verifier isolation: the reviewer gets `deny_read` on your
   `history://` and `deny_write` everywhere.
10. Resume a stuck child: `interrupt`, then respawn with `fork` of its
    tail plus your one-line correction.
11. Long-running probe: `check=` watches an external system; `needs_you`
    fires when it asks; your todo sits `blocked on child`.
12. Cost-shaped fan-out: `find_models` picks the cheapest for readers,
    yours for writers; `status().cost` is the fact you report.

**ipython description** (`crates/tools/src/ipython.rs:45`, 130 → about
700 bytes): "Execute Python in this session's kernel: one process, yours
alone, that boots on the first cell and keeps its variables across your
calls and across compaction (a snapshot revives them; a restart after a
hang says so and starts empty). Nothing here is shared with bash; the
cwd is. `await` works at top level; `rlm` is preloaded (`help(rlm.run)`);
`%%bash` runs a shell in the kernel's env and `%pip install x` adds a
package. Output over 64 KiB is cut; a cell has no timeout, so print
progress from a long loop. Children run their own kernels: `rlm.status()`
shows them, `rlm.put/get` and `kernel://<name>/<var>` move objects
between kernels whole, and a child reads yours through
`kernel://main/<var>`."

**identity.md** (:27-36): the plan line says "for work you hand out, a
check per task"; the RLM line becomes "RLM subagents from the kernel:
readers (`deny_write=["."]`) bring evidence, writers (`isolation=
'worktree'`) execute a todo with a check; `rlm.status()` shows them" with
the existing awaited example; the environment list (:38) gains `kernel`
and the new `children` shape.

**Tests and gates.** `request_budget` numbers move (`--update` in its own
commit; the row names the sections per D137); `check_prompt_examples.py`
passes; new `crates/runtime/tests/prompts.rs` pins that every identifier
an `orchestrate.md` example uses is defined in that example; `ext_e2e`
attach tests pass. Signals: `kernel_cells` (ipython calls), `shell_cells`
(`%%bash` cells), `children_spawned` (cell code containing `rlm.run(` or
`rlm(`), `readers_spawned` (with `deny_write`).

### S11 · Measurement: one paid rerun, ledger rows

Fixtures k=3 (`evals/run.py --live`, about five cents), then the slice k=3
(`TBV4_ATTEMPTS=3 sh evals/drivers/tbv4_baseline.sh`, about fifty cents at
catalog price), scored per finished job with `evals/axes.py <job dir>`
while the driver runs, the driver killed on a runaway. Rows 0024 and 0025
name: `spiral_cut`, `artifact_refused`, `closure_refused`, `gate_waived`,
`evidence_shape_refused`, `reduced_results`, `pointer_never_read`,
`length_forced` (0), `kernel_dead` (0), `module_missing`, `kernel_cells`,
`shell_cells`, `children_spawned`, `readers_spawned`, cost against row
0023. Expected: photonic writes a file on every attempt; near-misses gain
their last check where a checker exists (cargo, bun; heat-pump has none);
cost near row 0021's $0.45; `kernel_cells` > 0 on foodstuff and photonic;
pass 4/18 to 6/18. A larger jump is a model change, not this plan.

## Not built, with the reason

- **Renaming `rlm` to `child`.** Deferred by the user; a later row.
- **`YI_INSTALL_UV=1` in the adapter.** A harness-only switch; the ladder
  makes the shipped binary boot on the image's python3.
- **Embedding wheels in the binary.** Tens of megabytes; the trial has
  network for the provider, so pip has it too.
- **A `delegate` or `kernel` skill.** Not in the binary; the protocol
  fragment attaches on deterministic signals and did in every trial.
- **Bytes over the comm channel for objects.** The family dir is the
  transport; both kernels share the disk.
- **A shared kernel between agents.** One process per session stays; a
  child's crash must not be the parent's.
- **Computed disjoint ownership between siblings.** `deny_write` is
  authored from the plan's scopes; a computed intersection is a later row
  if the rerun shows a collision.
- **A child stuck-judge.** `stuck` is read from records the loop writes.
- **`cwd`/`timeout` kwargs on `rlm.run`.** The closed set is a design;
  the guidance names the real spellings.
- **Effort sandwich.** Effort is part of the cached prefix; the GLM route
  ignores `reasoning.max_tokens`.
- **A stop-time "read the tail" nudge.** Scaffolding around the reducer
  defect S3 fixes.
- **Evidence checked against the store.** A `done` rides with its check
  and the result may persist after the call; the shape check has no
  ordering hazard.
- **A hard closure block; model-family gating of the cutter; a relief
  model or reasoning off.** The user's calls and the endpoint's refusal.
- **A fixture task that spawns children.** The faux provider cannot
  script a kernel; S8/S9 are proven by their e2e tests and the manual
  check below.

## Open questions (answered at implementation, each a line in its ADR)

1. Does harbor charge `install()` to its own phase or to the agent timer?
   Decides whether K5 builds the venv at install or leaves it to prewarm.
2. Does a `set` that re-sends an existing `[x]` appear in the corpus
   often enough that refusing new `[x]` marks changes the seeding flow?
   (211 `set` calls; count the newly-closed ones before landing S6.)
3. The spiral constants: omp's or one notch up, from the calibration.

## Verification per stage

`just check` in the stage's worktree (lint, guardrails incl.
`evals/selftest.py` and `extract.py --selfcheck`, tests); focused:
`cargo nextest run -p yi-ai` (S1, S7); `-p yi-kernel` with
`YI_KERNEL_VENV` on a temp dir (S2); `-p yi-tools` (S3, S4);
`-p yi-runtime --test todo_coupling --test todo_e2e` (S5, S6);
`-p yi-loop` (S7); `-p yi-runtime --test subagent_e2e --test fetch_e2e
--test environment` plus `uv run pytest python/yi_runtime/tests` (S8, S9);
`-p yi-runtime --test request_budget --test ext_e2e --test prompts` and
`python3 scripts/guardrails/check_prompt_examples.py` (S10). Faux smoke
after S5, S7 and S10: `python3 evals/run.py --dry --binary target/debug/yi
--model faux/faux-1`.

Manual: S2 in a container from one v4 image (`docker run --rm -it` on the
photonic image with the musl `yi` bind-mounted, no `uv`): `yi doctor
--fix` shows `kernel-toolchain` green on python3 and `kernel-boot` under
30 s; `yi ask --here --yolo --model faux/faux-1 "run print(2+2) in
ipython"` returns 4 with `kernel: ready` in the environment. S8/S9 on
this repo: `yi ask --here --yolo` with a cell that spawns two readers
with `deny_write=["."]`, `put`s their findings, prints `rlm.status()`,
and the environment line shows both states change. S7: the calibration
numbers in the ADR and one live `yi ask` on the `search-loop` fixture
showing a `length_redrive` with `cut: true` before the paid run.
