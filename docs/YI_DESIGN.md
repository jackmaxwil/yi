# Yi design

Yi is a coding agent written in Rust: one `yi` binary, a persistent Python kernel beside it, and
sessions stored in Pi's v4 session JSONL format. This document states what the code does. §3
names the primitives; each primitive section lists its invariants, owner, state, shapes and the
D-rows in [ARCHITECTURE.md](ARCHITECTURE.md) that settle it.

## 1. Scope
### 1.1 What Yi is
| Topic | Decision | Settled by |
|---|---|---|
| Language | Rust workspace of 16 crates (§2); every dependency declared once in `[workspace.dependencies]` (§18) | D31 |
| Session file | Pi's v4 session JSONL format: a header, then one `Mutation` per line; any other version is rejected (§4.1) | D32 |
| Pi RPC | `yi rpc` serves Pi's RPC JSONL command names plus Yi's own (§17.1) | |
| Providers | Three wire APIs (Anthropic messages, OpenAI completions, OpenAI responses) plus `faux`; a compiled-in model catalog (§5) | |
| Login | `yi login` / `yi logout` in `yi-oauth`; no provider identity is compiled in | D191 |
| MCP | A one-shot CLI (`yi mcp`), compiled into every build, refused unless `mcp.enabled` is true; the agent reaches it through `bash` and the kernel, never a registered tool (§7.6) | D36, D71 |
| Python | A Jupyter kernel over ZeroMQ, its package embedded in the binary, its toolchain a pinned, verified uv (§9) | D156, D238 |
| Permission | Modes and rules, a per-segment command classifier, a catastrophic denylist, an optional model reviewer, Seatbelt containment on macOS (§8) | D81, D205 |
| Worktrees | A root session claims a git worktree slot unless `--here`, `lanes.enabled: false` or no repository (§14) | D119, D203 |
| ACP | v2 only; a lower `protocolVersion` gets a version-mismatch error; the wire is hand-rolled (§17.2) | D1, D40 |
| Workspace | `yi serve` owns sessions; `yi console` is an ACP client over its socket; bare `yi` on a terminal opens the console, `--solo` the TUI (§17) | D95, D118 |

### 1.2 Not in scope
A top-level feature enters only when one leaves, and this list changes in the same commit.
A resident MCP client or MCP as a registered tool · ACP v1 · automatic skill creation,
refinement or self-extension · embeddings and semantic search · resident LSP or DAP servers · a
JavaScript runtime or Node sidecar · embedding `yi-runtime` through N-API or WASM · document
converters compiled into the binary · Windows · dynamically loaded plugins (extensions are
in-process Rust, §6) · a GUI · browser automation, computer use, email, or any capability that
is not a coding agent · server-side terminal-frame streaming, per-pane PTYs or blit encoding in
the console.

## 2. Crates and dependency direction
A folder `crates/x/` is the crate `yi-x` (`scripts/guardrails/check_manifests.py`). The internal
dependency graph is the allowlist in `scripts/guardrails/boundaries.toml`; `check_boundaries.py`
fails on an undeclared edge, an unlisted crate or a stale entry. Size ceilings are in §18.6.

| Crate | Owns | Internal deps |
|---|---|---|
| `yi-types` | Serialized shapes: messages, events, entries, wire, config, plan, mail, lane, url | none |
| `yi-loop` | `run_loop`, `LoopConfig`, `AgentTool`, interrupt signal and soft-interrupt queue, tool-name repair | yi-types |
| `yi-ai` | Provider streams, model catalog, retry, SSE | yi-types, yi-oauth |
| `yi-oauth` | PKCE, loopback callback, token store, `login`/`logout` | yi-types |
| `yi-orb` | Orb geometry and kitty-graphics painter | none |
| `yi-session` | Session store: JSONL and in-memory repos, ids, queries | yi-types |
| `yi-context` | Token accounting, compaction cut point, context assembly | yi-types |
| `yi-permission` | Modes, rules, command classifier, catastrophic denylist, review ledger | yi-types |
| `yi-tools` | `Tool` trait, builtin tools, hashline edits, bash jobs, sandbox, checkpoints | yi-types, yi-permission |
| `yi-mcp-cli` | The `yi mcp` one-shot JSON-RPC client | yi-oauth, yi-types |
| `yi-kernel` | Jupyter client: connection file, ZMQ framing, HMAC, snapshot, uv install | yi-types |
| `yi-runtime` | `AgentSession` and everything that composes the crates above | yi-types, yi-loop, yi-ai, yi-session, yi-context, yi-permission, yi-tools, yi-kernel |
| `yi-acp` | ACP v2 server and the `yi serve` daemon | yi-types, yi-runtime |
| `yi-tui` | The ratatui shell | yi-types, yi-runtime, yi-orb |
| `yi-console` | The multi-pane workspace, an ACP client of the daemon | yi-types, yi-tui |
| `yi-cli` | The `yi` binary, composition root, `yi rpc`, `yi ask` | yi-types, yi-runtime, yi-acp, yi-tui, yi-console, yi-mcp-cli |

`yi-tui` and `yi-console` are optional in `yi-cli`, behind its default feature `tui`. Rules:
- `yi-types` depends on `serde`, `serde_json`, `sha2` and `thiserror` only: no async, filesystem
  or network code. It holds per-module error shapes, never a workspace error enum.
- `yi-loop` depends on `yi-types` alone among Yi crates. `run_loop` is infallible: it returns
  the new messages, and failures arrive as events and `ToolOutcome { is_error }`. The one
  `Result` in its public API is `AgentTool::validate`.
- `yi-runtime` is the only non-test constructor of `LoopConfig`.
- Surfaces (`yi-acp`, `yi-tui`, `yi-console`, `yi-cli`) never depend on `yi-tools`, `yi-ai` or
  `yi-permission`; `yi-console` does not depend on `yi-runtime`.
- Only `yi-cli` depends on `yi-mcp-cli`.
- Advisor, schedule, goal, subagent, mailbox, lane, plan, fetch, memory and prompt extensions are
  modules of `yi-runtime` (`crates/runtime/src/lib.rs`), not crates.
- `python/yi_runtime` (distribution `yi-runtime`) has two import packages: `rlm`, the host
  bridge, and `yi`, the plan library (§9). `python/skills` holds the kernel skills.

## 3. Primitives and composition
| Primitive | What it is | § |
|---|---|---|
| Session | An entry tree on disk plus a loop turn fed by one ordered run queue | §4 |
| Provider | A model stream behind `StreamFn`, chosen by role from the catalog | §5 |
| Prompt fragment | A ranked system-prompt slot, or fenced yard text, attached by an extension | §6 |
| Tool | A registered, byte-priced table entry the model calls | §7 |
| Permission | A decision per tool call: allow, contain (sandboxed), ask or deny | §8 |
| Kernel | A persistent Python process that reaches the host only through named host requests | §9 |
| Url | The one reference type, `scheme://path#fragment`, resolved by `fetch` | §10 |
| Child | A family member admitted under a lease, ending in one typed exit | §11 |
| Envelope | A message written to the receiver's session store before delivery | §12 |
| Plan and contract | A hash-chained op journal over todos; a done-predicate over a frozen attempt | §13 |
| Lane | A leased git-worktree slot with a moving hand-back | §14 |

| Capability | Composed from | Settled by |
|---|---|---|
| Worktree-first root session | Session + Lane | D119, D203 |
| `/land` a session to a merged PR | Lane + landing state events | D144 |
| Delegate a todo to a worktree child | Plan + Contract + Child + Lane (candidate, then integration) + Envelope | D195, D224, D225 |
| `fork_join` / `scatter` | Kernel (`yi` library) + Plan + Child | D212 |
| Review pod | Kernel shapes + Child (reader) per brief + arbiter + Contract | D217 |
| Judge a contract item | Contract (`judge` decider) + Child (jurors of other model families) | D216 |
| Service with a stable name | Child (service standing) + restart policy + Envelope | D218 |
| Child asks the human | Envelope (request to parent) + child flag | D233 |
| Shared family objects | Kernel `rlm.put` / `rlm.get` + Url (`family://`) | D164 |
| Paged reading of anything | Url + `fetch` pages | D213 |
| Checker after every covered write | Contract (`covers`) + Tool | D228 |
| Heartbeat | Schedule + Session run queue (steer or follow-up) | |
| Next-step hints | Tool result + procedural graph | D219 |

## 4. Session
An append-only log on disk (§4.1) plus one in-memory run (§4.2-§4.5).

### 4.1 Session file
The file is Pi's v4 session JSONL format, a byte-level contract: a header, then one mutation/line.
- Line 1 is `JsonlV4Header`; a version other than 4 is rejected. Each later line is a `Mutation`
  tagged `kind`: `entry` (optional `lane`, flattened `Entry`), `record` (`LaneRecord`), `lane`
  (`seq, lane, leafId`) or `fact` (`seq` + `name|label|goal|plan`).
- The store stamps `parentId`, `seq`, `timestamp`, appends the line, then applies it; no fsync.
- Lanes are named leaf pointers, `main` first; fork, rewind and navigate create or move a lane,
  never copy or delete. `LaneRecord`s are preserved; Yi keeps no operation log. Goal and plan are
  facts outside the entry tree, so compaction cannot drop them.
- A last line failing as JSON syntax or EOF is a torn write: the file is atomically rewritten to
  its valid prefix. Any other bad line fails the load.
- `pi_v4_fixtures_roundtrip_byte_identical` re-serializes every line and whole file of
  `crates/types/tests/fixtures/*.jsonl` byte-identically; `JsonlRepo` and `MemRepo` pass one
  suite, `crates/session/tests/conformance.rs`. Ids are validated before any path join.
- Owner: [`crates/session/src/`](../crates/session/src/); Settled by: D32, D33, D78, D115
- State: `SessionRepo { create, open, list, delete, fork }` → `Arc<Mutex<SessionStore>>`;
  `Entry { Message, ModelChange, ThinkingLevelChange, ActiveToolsChange, Compaction,
  BranchSummary, Custom }`; `Fact { Name, Label, Goal, Plan }`. Yi's `custom` entries:
  `checkpoint` (§7.7), `todo`, `todo_intercept`, `plan_op` (§13), `fetch` (§10), `agent_message`,
  `agent_message_read`, `human_answer` (§12), `lease` (§11), `ext_state`, `ext_record`.
- Shapes: [`wire.rs`](../crates/types/src/wire.rs), [`entry.rs`](../crates/types/src/entry.rs),
  [`record.rs`](../crates/types/src/record.rs); `~/.yi/sessions/--<cwd, / \ : as ->--/
  <createdAt>_<id>.jsonl`; a child's is a flat file in its own directory.

### 4.2 Loop
`yi_loop::run_loop` runs turns until the model calls no tool and nothing is queued; yi-types only.
- `async fn run_loop(&mut LoopContext, Vec<AgentMessage>, &LoopConfig, &InterruptSignal, emit,
  &impl StreamFn) -> Vec<AgentMessage>`. Failures are values: `StopReason::Error` or `Aborted`
  ends the run; `AgentTool::validate` is the one `Result`. Every exit emits `AgentEnd`.
- Per request: `maybe_compact` → `transform_context` → `convert_to_llm` → `StreamFn::stream`.
  Steering is taken after each turn; follow-ups only when the loop would end.
- A maximal run of `Parallel` calls executes concurrently; the first `Sequential` call closes it.
  `StopReason::Length` fails every tool call in the message unrun.
- An unknown tool name is repaired only by a unique normalized match (case, separators,
  namespace prefix, `tool` affix). Text opening `<tool_call>` with no call is `UNPARSED_MARKUP`.

| guard ([`run.rs`](../crates/loop/src/run.rs)), a hidden `custom` message | rule |
|---|---|
| `repeat_break` | window of 6 signatures (tool batch by name+args, or text cut at 256 chars); sent once when a batch has 3 copies and the last 3 turns repeated; 6 copies end the run; bash job polls never count; a follow-up clears the window |
| `length_redrive` | after a tool-less `Length` stop; 3 consecutive length stops end the run |
| reasoning cut | 48,000 reasoning chars set the cut flag (§4.5); kept as a bare `Length` stop and re-driven; 6 cuts per prompt end the run |
| `stream_retry` | once per error streak, when the error turn showed no text and no call |

- Owner: [`crates/loop/src/`](../crates/loop/src/); Settled by: D145, D163, D175, D178, D179, D197
- State: [`LoopConfig`](../crates/loop/src/config.rs) (the hooks above);
  `ExecutionMode { Sequential, Parallel }` (default `Parallel`); `NextTurn { model, thinking }`.
- Shapes: `AgentEvent { AgentStart, AgentEnd, TurnStart, TurnEnd, MessageStart, MessageUpdate,
  MessageEnd, ToolExecutionStart, ToolExecutionUpdate, ToolExecutionEnd, PermissionRequested,
  PermissionResolved, ChildUpdate, LandingState }` ([`event.rs`](../crates/types/src/event.rs));
  `MessageUpdate` carries one delta, folded by `event::apply`; usage rides `MessageEnd`.

### 4.3 Run queue
- `prompt` returns at admission, `SessionError::Busy` while running. `steer` holds mail, host
  notices and steers; `follow_up` holds follow-ups; both in memory.
- `deliver(message, wakes)` answers under the status lock: `Queued`, `Woken` or `Inboxed`.
- An envelope queues once; a sender's newer progress replaces its queued one; an entry whose
  `news` test is false is dropped unread. On resume, unread `agent_message` entries re-enter.
- Every run end goes through `settle`: under the status lock it starts the next turn on the oldest
  waking entry, else a follow-up unless aborted, else goes `Idle`.
- Owner: [`crates/runtime/src/session/run.rs`](../crates/runtime/src/session/run.rs);
  Settled by: D230
- State: `Status { Idle, Running }`; `Queued { message, wakes, news }`;
  `Delivery { Queued, Woken, Inboxed, Answered }` ([`mail.rs`](../crates/types/src/mail.rs))

### 4.4 Context and compaction
- Path: `main`-lane entries → `yi_context::project` (at attach; non-message entries dropped,
  starts at the latest `Compaction`'s summary + `retained_tail`) → session messages →
  `transform_context` (environment block) → `yi_context::convert_to_llm` → provider.
- `convert_to_llm` wraps `custom` kinds `heartbeat_prompt, advisory, goal_prompt, ledger_prompt,
  plan_dispatch, reminder` as `<yi_internal_context source="…">`; compaction drops them.
- Due when scheduled (`/compact`, `compact.run`) or when tokens past the server-observed prefix
  exceed window − 16,384 (last usage + chars/4 after it). The cut keeps 20,000 recent tokens,
  never at a tool result; up to 64,000 tokens of summarized user text join the tail.
- The summarizer replays system prompt and converted messages plus a trailing directive, no
  tools; model `models.summarizer`, else the session's; a failure retries once without the first
  quarter, then yields an empty summary. The summary leads with `<yi_compact_view>`.
- A compaction whose entry fails to write is not applied: history and window stay as they were, a
  `[compaction not saved: …]` notice reaches the model, and auto-compaction pauses until an
  explicit `/compact` succeeds.
- Owner: [`crates/runtime/src/compaction.rs`](../crates/runtime/src/compaction.rs),
  [`crates/context/src/`](../crates/context/src/)
- Shapes: `Entry::Compaction { summary, retained_tail, tokens_before, details?, usage? }`,
  [`CompactionDetails`](../crates/types/src/compaction.rs) `{ readFiles, modifiedFiles, window?,
  extra }`. Settled by: D115

### 4.5 Interrupt
- `abort` fires `InterruptSignal` (sets `fired`, bumps `epoch`, wakes waiters); a run clears
  `fired` only if the epoch is the one read at admission. A streaming request ends with its
  partial as `Aborted`; an unstarted call returns `ToolErrorKind::Aborted`; running tools see
  their cancel flag; no later batch starts.
- `cut` is a separate flag, set by the loop and read by provider pumps between SSE events.
- Owner: [`crates/loop/src/interrupt.rs`](../crates/loop/src/interrupt.rs); Settled by: D163

## 5. Provider
yi-ai streams one assistant message per request as `AssistantMessageEvent`s on a 256-slot channel.
- Dispatch by `model.api`: `anthropic-messages` (`anthropic`), `openai-responses` (`openai`,
  `openai-codex`), `openai-completions` (`openrouter`, `google`); any other api is faux. Catalog:
  deflated bundle overlaid by `~/.yi/catalog/<provider>.json`. Model: `--model`, else
  `models.primary`, else `model`; none is built in.
- Credentials: env key (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`,
  `GEMINI_API_KEY`), else the yi-oauth store; OAuth refreshes before use, falling back to disk.
- Retry before the response body only: 408, 409, 429, 5xx, transport errors; 3 attempts, 300 s
  wall; `retry-after(-ms)` if ≤ 60 s, else 0.5 s·2ⁿ ≤ 8 s, no jitter; read timeout 60 s.
- `leak::recover_in` in both mappers' `finish` turns each well-formed text `<tool_call>` block
  into a `ToolCall` (id `leak-N`) and `Stop` into `ToolUse`.
- Stable cached prefix: `read`'s description fixed per session, content-hashed fence ids, a
  minute-cached `time:`; adapters set `cache_control`, `prompt_cache_key`, cache retention.
- yi-oauth: PKCE S256 from `/dev/urandom`, a `127.0.0.1` loopback, `login`/`logout`; tokens in
  `~/.yi/providers/tokens/<provider>.json` (0600, tmp+rename), refreshed 30 s before expiry under
  a per-provider flock; profiles are the user's `~/.yi/oauth/<provider>.json`, Yi ships none.
- Owner: [`crates/runtime/src/provider.rs`](../crates/runtime/src/provider.rs),
  [`crates/ai/src/`](../crates/ai/src/), [`crates/oauth/src/`](../crates/oauth/src/)
- State: `models { primary, summarizer, advisor, autoReview }` in `~/.yi/config.json`
  ([`config.rs`](../crates/types/src/config.rs)), advisor and autoReview off until named;
  [`RetryPolicy`](../crates/ai/src/retry.rs). Shapes: [`model.rs`](../crates/types/src/model.rs).
- Settled by: D34, D57, D128, D191, D197, D231

## 6. Prompt
The system prompt is a table of ranked slots plus a yard of fenced external text, owned by an
extension `Host` whose synchronous extensions turn session events into effects.

- `PromptState::assemble` joins three blocks with `SYSTEM_BLOCK_SEPARATOR`: ranks ≤ `Doctrine`,
  ranks > `Doctrine`, the yard. `ext::install` attaches identity, doctrine, the permission-mode
  fragment, user system text and schema instruction (text compiled in from
  `crates/runtime/src/prompts/`), then registers `project-resources` (skills catalog), `pack`
  (`lang-rust`, `~/.yi/extensions/*.json`), `orchestrate`, `grid`, `route-telemetry`, `memory`.
- Yard text never enters a slot. It renders as `<<<yi-external <id> source="…" trust="…">>>`,
  `<id>` = first 16 hex of the text's content hash; `<<<` is escaped, control chars stripped.
  Project instruction files (`AGENTS.md`, `CLAUDE.md`) and project skills render here. Project
  packs load only when `~/.yi/trust.json` (`yi trust`) grants their content hash.
- The table persists as `custom{ext_state}` on change and is restored on resume.
- The environment block is a `host_user` message appended per request by `transform_context`,
  never stored: `cwd`, `files` (20), `landing`, `todos`, `time` (per minute), `deadline`,
  `platform … · host <line>`, `model`, `context`, `kernel`, children. `host` comes from
  `host::probe_at` (DMI, cpuinfo, container markers, SSH, WSL), probed once; SSH is re-read.
- Reminders fire on evidence: `orchestrate` attaches silently at turn end only after an edit and
  more calls than its lever; `edit_before_read`, `files_matched`, `failed_check_after_edit` remind.
  `RuleEngine` reads `~/.yi/rules` and `.yi/rules` (none built in); ≤ 2 `skill://` hints a turn.
- Next-step lines: `affordance::render` over the compiled-in `graph.json` walks 2 hops from the
  last call over edges whose condition is `always` or a host-asserted fact from the closed
  `PREDICATES`, by weight, ≤ 2 `next:` lines (3 for `todo`). `Graph::check` bounds edges (400),
  out-edges (8), guidance (160 B), pitfalls (3 × 120 B), and forbids self edges.
- Levers: tunables are read via `levers::get()`, `DEFAULT` unless `init` (first statement of
  `build_session`) loaded `YI_LEVERS`, read only with `--eval`; a bad file is `refusal:config`.
- Skills catalog: `window × 4 / 50` bytes, clamped 8-32 KiB; descriptions clip to 120, then 60
  characters, then names fold into `+N more:`. The request budget is §7.1.
- Owner: [`ext/`](../crates/runtime/src/ext/mod.rs), [`env`](../crates/runtime/src/environment.rs),
  [`rules`](../crates/runtime/src/rules.rs), [`affordance`](../crates/runtime/src/affordance.rs),
  [`levers`](../crates/runtime/src/levers.rs), [`host`](../crates/runtime/src/host.rs)
- State: `Rank { Identity, Doctrine, Mode, Lang, Protocol, Tool, User, Catalog, Schema }`, `Slot
  { rank, name }`, `Trust { Granted, Untrusted }`, `PromptState { slots, yard }`, `Event {
  SessionStart, PromptSubmitted, ToolCall, ToolResult, TurnEnd, Usage, Compacted }`, `Effect {
  AttachFragment, DetachFragment, AttachExternal, Remind, Record }`.
- Shapes: [`graph.rs`](../crates/types/src/graph.rs) (`Graph`, `Edge`, `Relation`, `Predicate`).
- Settled by: D138, D139, D187, D188, D196, D209, D219, D220, D231.

## 7. Tool
A `yi_tools::Tool` adapted to the loop's `AgentTool`; the set is fixed at session build.

### 7.1 Table and trait
| Read | Write | Exec | Ledger |
|---|---|---|---|
| `read`, `grep`, `get_context`, `ask_user` | `edit`, `write` | `bash`, `ipython` | `plan`, `todo` |

- `builtin_tools_with` builds `read edit write grep bash get_context`; `session_tools` adds exec
  tools from `~/.yi/tools` (`--schema`, JSON on stdin, exit ≠ 0 is an error); wiring pushes
  `ipython`, `plan`, `todo`, `ask_user`. `check_request_budget.py` prices system block plus tool
  table against a shrink-only baseline and locks a sha256 per tool in `tool_surface.json`.
- `Tool { name, description, schema, kind, kind_for, freeform, irreversible, validate, preview,
  execute -> ToolOutput }`. `kind_for` decides per call; Read runs parallel, the rest sequential.
  The adapter runs rule check, reviewer wall, permission gate (§8), next-step lines (§6).
- Owner: [`tool.rs`](../crates/tools/src/tool.rs), [`tools.rs`](../crates/runtime/src/tools.rs).
- State: `ToolKind { Read, Write, Exec, Ledger }`, `ToolContext { cwd, cancelled, recovery_dir,
  auto_background, sandbox, deny_read, call_id }`. Settled by: D117, D137, D188.

### 7.2 Editing
Views are `N:TEXT` under `[PATH#TAG]`, `TAG` = 4 hex of xxh32 over trailing-ws-stripped text.
Ops: `PUT N.=M:`, `PUT N*:`, `PUT <N:`, `PUT >N:`, `PUT >N*:`, `@name` pastes, `CUT`, `REM`,
`MV DEST`; body rows `+TEXT`. Stale numbers remap with a `rebased` warning; a cited line that
changed is rejected. A header one stray mark from valid is repaired, with a warning, only when
every one-mark deletion that parses names the same op. Three no-op edits in a row refuse.

- Owner: [`hashline/`](../crates/tools/src/hashline/mod.rs). Settled by: D117, D237.

### 7.3 bash and jobs
The interpreter is `bash` if `command -v bash` succeeds, else `sh`, probed once. Timeout 300 s
default, 600 s cap. Jobs: ≤ 32, LRU with the 8 newest protected. `is_error` = exit ≠ 0 or
cancelled. Output ≤ 8,192 bytes, or with `-v --verbose --nocapture --porcelain -la -C`, is whole;
past it: strip ANSI, compress, filter (cargo; grep/rg/ag 60 lines; else head 80/tail 40), kept
only if shorter. Lossy output is tee'd to `~/.yi/tool-output/` and ends `[full output: <path>]`;
with nowhere to tee, raw text returns. A contained call runs under Seatbelt (§8).

- Owner: [`jobs.rs`](../crates/tools/src/jobs.rs), [`reduce.rs`](../crates/tools/src/reduce.rs).
- Settled by: D161, D221.

### 7.4 read
`path` is a file, directory or glob; `find`, `offset`/`limit` (2000 lines), `ranges`, `pages`
select; byte budget 50 KiB floor, 512 KiB ceiling. A head with a zip, OLE2, `%PDF-` or `{\rtf`
marker goes to the kernel venv's converter; the Markdown lands read-only in `~/.yi/converted/`,
refused by `edit`/`write`; csv is never converted. The description names the venv's formats.

- Owner: [`document.rs`](../crates/tools/src/document.rs). Settled by: D171, D231.

### 7.5 Loud caps
Every cap that shrinks a model-facing view is named at the cut as a `[…]` row: kept/total, the
cap's name and value, the call that gets the rest. A window or filter the model asked for is not
a cap. Rule: [`.ruler/045-loud-caps.md`](../.ruler/045-loud-caps.md); test:
`every_cut_view_names_its_cap` in [`tools.rs`](../crates/tools/tests/tools.rs).

### 7.6 MCP CLI
MCP is a CLI, never a tool. `yi mcp` runs one-shot: `connect <server> @s`, `close`, `restart`,
`login`, `logout`, `grep`, `skill`; per session `tools-list|get|call`, `resources-list|read`,
`prompts-list`, `ping`. It speaks JSON-RPC itself, only yi-cli depends on it, `mcp.enabled` gates
it. A bare name resolves in `~/.yi/mcp.json`, `.mcp.json`, `.vscode/mcp.json`, `.cursor/mcp.json`.
The kernel shells out: `rlm.mcp.list_tools|call_tool|reload|close` run `$YI_BIN mcp … --json`.

- Owner: [`mcp-cli`](../crates/mcp-cli/src/lib.rs). Settled by: D36, D71.

### 7.7 Checkpoints
A shadow gitdir `~/.yi/checkpoints/<xxh32 of project>/`, the project as work tree, one lock per
gitdir. `capture` = `add --all` + `write-tree`. `restore` checks out every path changed since the
tree and deletes paths created since, whoever wrote them. Turn start and end capture into
`custom{checkpoint}`; `/undo` and `yi undo` restore the last turn start. Without git, a no-op.

- Owner: [`tools`](../crates/tools/src/checkpoint.rs), [`types`](../crates/types/src/checkpoint.rs).

### 7.8 Skills
Roots `{.yi,.agents,.pi,.claude}/skills` under cwd, then home; first root wins a name;
`<name>/SKILL.md` walked 2 levels. Frontmatter at discovery, body via `read`; `$name` arms the
skill as a rule. Bundled: `skills/yi` (`just install-skills`), and Python skills `attach-image`,
`compact`, `goal`, `memory` shipped in the binary for the kernel venv.

- Owner: [`skills.rs`](../crates/runtime/src/skills.rs). Settled by: D139.

## 8. Permission
A pure `decide` over the call, mode, rules, grants, holds and catastrophic context.

- Modes `Ask`, `Auto`, `Yolo`; default `Auto` (`--confirm`, `--auto`, `--yolo`); one prompt
  fragment each. Order: catastrophic (every mode), configured deny, session rule or grant,
  configured allow/ask, hold, mode. `Auto` allows reads, in-tree writes and provably safe
  commands, contains the unproven, asks for destructive and egress segments. Commands are
  decided per segment; an unparseable command is its own input.
- `git_dirs` reads `.git`, `gitdir:` and `commondir` with bounded reads, trusting a pointer only
  to a `HEAD`; catastrophic matching covers every spelling and reads through wrappers.
- `refused_scopes` records a contained failure by program and verb; a later use asks. "Always
  allow" keeps a `Grant` (directory, tree root, program+verb), in memory, ≤ 1024 rules.
- Sandbox: Seatbelt `/usr/bin/sandbox-exec`, macOS only; writable cwd, git dirs minus
  `hooks config commondir gitdir`, session dir, tmp; reads deny `~/.ssh ~/.gnupg ~/.aws ~/.kube
  ~/.docker`; no network. Without it, `Contain` becomes a reviewable `Ask`.
- With `models.autoReview` set, a reviewable ask goes to the reviewer (30 s); non-allow denies
  with a request id `ask_user` replays; `ActionLedger` (256) makes an approval single-use.
- Owner: [`decide`](../crates/permission/src/decide.rs), [`sandbox`](../crates/tools/src/sandbox.rs)
- State: `PermissionMode { Ask, Auto, Yolo }`, `Decision { Allow, Contain, Deny, Ask { title,
  description, reviewable } }`, `Class { Safe, Destructive, Egress, Unknown }`.
- Shapes: [`types`](../crates/types/src/permission.rs). Settled by: D15, D26, D81, D205, D206, D207.

## 9. Kernel
A persistent IPython process per session that reaches the host only through host requests.

- One `python -m ipykernel_launcher -f <connection file>` per session over Jupyter protocol 5.3
  on the pure-Rust `zeromq` crate; frames after `<IDS|MSG>` are HMAC-SHA256 signed.
- It boots on the first `ipython` cell, or in the background at session open under
  `kernel.prewarm` (default true); a failed prewarm is silent and the next cell reports it.
- It runs under the session Seatbelt profile plus `~/.yi/harness` and `~/.yi/mcp`; a cell's
  wall clock is `cell_ceiling`, default 600 s, started after boot.
- A host request is a comm on target `host.request`, dispatched once per comm id; the reply
  rides the control channel as `{status: "ok", ..}` or `{status: "error", error}`. An
  unregistered verb answers `host request type "X" is not available in this session`.

### 9.1 Bootstrap
- `python/yi_runtime` and `python/skills` ship deflated in the binary, unpacked on a new stamp.
- Toolchain: `uv` on PATH or `~/.local/bin/uv`, else `~/.yi/uv/<version>/uv`, else `python3`
  3.11+, else the pinned uv (`uv_install::PINNED`, a sha256 per target) downloaded and verified.
- The venv `~/.yi/kernel-venv-<8 hex>` is keyed by `RUNTIME_READY_CHECK` and the extras;
  `.bootstrap-version` holds the runtime identity (sha256 over the package and bundled skills),
  and a mismatch rebuilds it under the bootstrap lock; `YI_KERNEL_VENV`/`YI_KERNEL_PYTHON` override.

### 9.2 Host requests
`HostRegistry` maps a verb to an async handler; every registered verb, by owner in `runtime/src/`:

| owner | verbs |
|---|---|
| `kernel.rs` | `exec.spawn/tail/poll/kill/release` (the `bash()` handle), `mcp.config` (`{}`), `mcp.refresh` (error) |
| `wiring.rs`, `memory/mod.rs` | `fetch`, `history.grep`, `compact.run` (schedules only), `compact.status`; `memory.save/read/forget` |
| `subagent.rs` | `rlm.run` (returns at admission), `rlm.result/wait/status/list_subagents/delete_subagent/merge_worktree/discard_worktree/find_models`, `model.info`, parent-side `agent_message.send/request/list_agents` |
| `mailbox.rs`, `lease.rs`, `subagent/service.rs` | child-side `agent_message.send/request/list_agents`, `rlm.receive`; `rlm.interrupt`, `rlm.revoke`; `rlm.service` |
| `schedule/mod.rs`, `goal/mod.rs`, `plan/mod.rs`, `plan/request.rs` | `rlm_heartbeat.list/create/update/delete`; `goal.get/create/update`; `plan.get`, `plan.op` |

### 9.3 Python surface
- The preamble binds `rlm` (the module), `mcp`, `fetch`, `bash`. `@_public` builds `rlm.__all__`:
  async `host_request, run, service, find_models, list_subagents, delete_subagent, send,
  request, receive, followup, status, list_agents, wait, plan_op, interrupt, revoke, result,
  merge_worktree, discard_worktree, fetch`; sync `put, get, ls, bash`.
- A public coroutine a cell never awaits runs once in `post_run_cell` and prints its value; a
  mail call (`send`, `followup`, `interrupt`, `revoke`) is a task scheduled at the call.
- Blackboard `<sessions dir>/rlm-<pid>/family/`: `rlm.put(name, obj)` writes `<name>.dill` and
  `<name>.json` `{name, owner, at, bytes, type, serializer}`; `get` and `ls` read them.
- Kernel env: `YI_BIN`, `RLM_SESSION_DIR` (sessions dir for a root, the child's dir otherwise),
  `RLM_FAMILY_DIR`, `RLM_GLOBAL_HARNESS_STATE_DIR`, `RLM_DEPTH`, `RLM_MAX_DEPTH` (unread).
- Snapshot: `<kernel dir>/<store id>.kernel-state.{dill,json}`, dilled 1.5 s after an `ok` cell,
  flushed on dispose (≤ 5 s), restored at boot; ≤ 256 MiB, 16 MiB per variable.
- Owner: [`kernel/src/client.rs`](../crates/kernel/src/client.rs),
  [`bootstrap.rs`](../crates/kernel/src/bootstrap.rs),
  [`uv_install.rs`](../crates/kernel/src/uv_install.rs),
  [`runtime/src/kernel.rs`](../crates/runtime/src/kernel.rs),
  [`rlm/__init__.py`](../python/yi_runtime/src/rlm/__init__.py)
- Shapes: [`types/src/kernel.rs`](../crates/types/src/kernel.rs)
- Settled by: D87, D164, D171, D176, D211, D232, D235, D238

## 10. Url and fetch
`Url` is the one reference type; `Resolver` turns one into text and logs every read.

- `Url { scheme, path, fragment }` serializes as `scheme://path[#L<start>-<end>@<TAG>]`.
  Whitespace or an empty scheme or path fails the parse; an unknown scheme parses as `External`
  and fetch refuses it. Only `local` and `checkpoint` take a fragment; `TAG` is the whole-file
  xxh32 in four uppercase hex digits, and a live file that differs is refused `Stale`.
- `kernel` and `agent` are `Ephemeral`, the rest `Durable`. The wall (§11) runs first
  (`deny_url` by prefix, `deny_read` by path); `local` stays under the workspace or spill dir.
- A reply is `{url, text, hash, servedBy}`, logged in `FetchLog`. `offset`/`limit` page
  `local` by bytes on char boundaries, `history` by entries, `kernel` by chars, adding
  `next_offset` (null at the end); a zero `limit`, a non-integer, or another scheme is refused.

| scheme | path | serves |
|---|---|---|
| `local` / `user` | `<path>` / `<n>` | a workspace or spill file / the n-th user-attributed message |
| `kernel` | `<agent>/<var>` | a member's variable: repr ≤ 8192 chars, or dilled into the family dir |
| `plan` / `agent` | `<id>[/<slug>]` / `<name>` | a plan via its journal (§13) / a live child's transcript, else its reap pin |
| `history` | `<agent>[/<entry>\|/tail/N\|/since/<seq>][/custom/<type>]` | a transcript; `self` is the reader's |
| `checkpoint` / `mcp` | `<tree>/<path>` / `<server>/<uri>` | a checkpoint-tree file (§7.7) / an MCP resource (§7.6) |
| `family` / `tree` | `<name>` / `<agent>/<path>` | a blackboard sidecar / a member's checkout file, under `deny_read` |

- Owner: [`runtime/src/fetch/mod.rs`](../crates/runtime/src/fetch/mod.rs),
  [`runtime/src/fetch/schemes.rs`](../crates/runtime/src/fetch/schemes.rs)
- Shapes: [`types/src/url.rs`](../crates/types/src/url.rs)
- Settled by: D98, D164, D213

## 11. Child
A detached `AgentSession` admitted by `SubagentHost` under a lease, a wall and a standing.

- Every child enters through `SubagentHost::admit`; `rlm.run` answers
  `{rlm_child_id, next, name, session_dir, model}` at admission and the run proceeds detached.
- kwargs are a whitelist: `name, model, thinking, fork, isolation, deny_write, deny_read,
  deny_url, context, check, deadline_s, tokens, parent_close`; `fork=all` refuses a model.
- Admission refuses at lever `family.cap` (16) live sessions, at depth `rlm.maxDepth` (1,
  clamped 1..=3) but for a juror, at `family.max_children` (8) workers, and on a taken name.
- `Standing { Worker, Juror, Service }`: juror and service stand outside the worker cap; lease,
  wall and family cap bind all three. A service (`rlm.service(name, brief, restart=3)`)
  respawns on its `ChildRecord` after `Failed { Provider | KernelDeath }`, ≤ 10 per 10 min.
  A judge seats `policy.n` jurors from the cheapest other model family, else `Abstain` (§13).
- The wall only reduces and `under(parent)` makes it hereditary; it refuses at the tool adapter
  before permission, `deny_read` implies write-deny, and bash naming a denied path is refused.
- A lease is drawn under the roster lock; an ask past the parent's deadline less 30 s or its
  unreserved tokens is refused with both numbers. `rlm.revoke` journals `custom{lease}` and
  sends `cancel`; at grace expiry the child is `Repossessed` with its lane `Retained`.
- `ChildRecord::step` is the one writer of a record; `family::read_exit` the one reader of an
  exit. `retire` is the only exit from the roster: it refuses a worktree child with no
  disposition, aborts a live run, disposes the kernel, settles the lane, returns the lease and
  publishes one final `ChildUpdate`. A `worktree` lane goes back by `rlm.merge_worktree` (§14).
- State: `ChildExit { Completed, Failed { class }, Interrupted, Reaped, Repossessed }`,
  `FailClass { RefusedSpawn, Provider, KernelDeath, RedCheck, Deadline }`, `ChildStatus`,
  `ChildActivity`, `ChildFlag { NeedsYou, Stuck }`; `family.rs` holds `Cause` and
  `MemberState { Queued, Running, Finished, Failed, NeedsYou, Stuck, RepossessionPending }`.
- Owner: [`runtime/src/subagent.rs`](../crates/runtime/src/subagent.rs),
  [`wall.rs`](../crates/runtime/src/wall.rs), [`lease.rs`](../crates/runtime/src/lease.rs),
  [`family.rs`](../crates/runtime/src/family.rs)
- Shapes: [`types/src/subagent.rs`](../crates/types/src/subagent.rs) (`ChildUpdate` on
  `_yi/subagent_update`), [`types/src/lease.rs`](../crates/types/src/lease.rs) (`Lease`,
  `LeaseRecord`, `ParentClose`); child dirs `<parent rlm dir>/sub-<8 hex>`
- Settled by: D165, D210, D215, D216, D218, D234

## 12. Mailbox
A family message: an envelope in the receiver's inbox before delivery, then its one queue.

- The host fills `id`, `from`, `to`, `seq`, `sentAt` under the `Desk` lock (`seq` per pair).
- The inbox is `custom{agent_message}` in the receiver's own store, appended before delivery (a
  refused write refuses the send), read as `history://<agent>/since/<seq>/custom/agent_message`.
- A body over 16 KiB is refused whole; a sender holds ≤ 16 open requests. `progress` and
  `failure` go up only; `cancel` comes down only.
- One queue `Queued { message, wakes, news }` per session takes mail, notices and steers,
  presented at every message boundary. `inform` wakes only with `followup`; `progress` never
  wakes and the queue keeps a sender's newest one; every other kind wakes.
- A run ending `Completed` with a request open is steered once; a second such ending replies
  with its last text, `answeredBy = "final_text"`. A child's `ask_user` is a request to its
  parent, answered by `rlm.send(child, text, reply_to=id)`.
- `rlm.wait` clamps to 1-300 s and returns `state` (`asks`, `settled`, `moved`, `timeout`),
  a cursor and `causes`; `rlm.receive` takes queued envelopes into `agent_message_read`.
- A target resolves by exact name, else the one child named `.../<target>`; ambiguity refuses.
- State: `Kind { Inform, Request, Reply, Progress, Failure, Cancel }`,
  `Delivery { Queued, Woken, Inboxed, Answered }`.
- Owner: [`runtime/src/mail.rs`](../crates/runtime/src/mail.rs),
  [`mailbox.rs`](../crates/runtime/src/mailbox.rs),
  [`session/run.rs`](../crates/runtime/src/session/run.rs)
- Shapes: [`types/src/mail.rs`](../crates/types/src/mail.rs) (`Envelope`, `Receipt`)
- Settled by: D214, D230, D233, D235, D236

## 13. Plan and contract
A plan is a list of labeled todos with ordering edges, changed only by ops that `PlanEngine`
applies and journals. A todo's contract decides when it is done.

- The engine is the only writer. Every op, applied or refused, appends one digest-chained record
  (`sha256(prev ‖ canonical)`) to the root plan's journal, synced under the store lease, first.
- `plan.json` is the journal's checkpoint (32 KiB cap, schema-validated): a read regenerates it, a
  mutation refuses one that disagrees (`ExternalEdit`), one with no journal is `JournalMissing`.
- Todos are addressed by label; `after` edges order siblings, carry no data, are acyclic; ready is
  derived, never stored. A non-Active plan admits `view` alone (Done: also `retry`, `program`).
- `Actor::User` is minted only by `authority::confirmed`, from a permission prompt a tty process
  holds, for one request, valid 300 s. `yi plan` and the serve worker refuse user-only ops.
- Owner: [`crates/runtime/src/plan/ops.rs`](../crates/runtime/src/plan/ops.rs) (legality:
  `table.rs`).

State ([`doc.rs`](../crates/types/src/plan/doc.rs), [`op.rs`](../crates/types/src/plan/op.rs)):
- `TodoState { Pending, Running{by}, Blocked{on, note}, Done{output, resolution},
  Failed{cause, last}, Abandoned, Other }`, `BlockedOn { Child, User, External{probe}, Other }`,
  `PlanState { Active, Done, Superseded{by}, Abandoned, Other }`.
- `OpKind`, 23 kinds; the `plan` tool schema shows the first 15 (`MODEL_OPS`): `set, init, append,
  drop, block, unblock, reorder, add_edge, start, done, fail, retry, decompose, supersede, view`.
- `Actor { Owner, Child, User, Host, Engine }`. User may apply every op; Owner every op but
  `fuse_reset`, `resolve`, `accepted_by_user` and a resolving `repair`; Host `unblock, reconcile`;
  Child `view, submit`; Engine `start, submit, done, fail, retry`.

| From | Op → To (`STEPS`); any other pair is refused |
|---|---|
| Pending | start → Running · block → Blocked · drop → Abandoned · add_edge → Pending |
| Running | done → Done · fail → Failed · block → Blocked · decompose → Running · accepted_by_user → Done |
| Blocked | unblock → Pending · drop → Abandoned · add_edge → Blocked · accepted_by_user → Done |
| Done | add_edge → Done |
| Failed | retry → Pending · add_edge → Failed · accepted_by_user → Done |

- Shapes: `.yi/plans/<root>/ops.jsonl` (gitignored) of `JournalRecord`
  ([`ledger.rs`](../crates/types/src/plan/ledger.rs)), `.yi/plans/<id>/{plan.json, artifacts/}`,
  `.yi/plans/.lease`, `.yi/schemas/plan.schema.json`. Surfaces: the `plan` tool, host requests
  `plan.get` and `plan.op`, RPC `plan`, `yi plan`. The session holds `Fact::Plan` as a pointer.
- Settled by: D192, D193, D223.

### 13.1 Contracts
A `Contract` ([`contract.rs`](../crates/types/src/plan/contract.rs)) is 1..=16 items
`{id, critical, weight, decider}` with a `threshold`, a `min_coverage` and `covers` globs.

- `Decider { Cmd, Schema, Example, Judge }`. `ContractClass { Writer, Reader, Inline, Service }`
  sets a floor (a critical `cmd`/`example`, `schema`, or either); `Service` is refused.
- `start` freezes the contract by digest with the attempt. `done` commits
  `verification_requested`, releases the lease, runs the verifier (own deadline, emptied env, a
  copy of the tree), re-acquires, compares the whole token, and commits the verdict with
  `Done{VerifiedDone}`, `done_refused`, or `verification_stale` for a moved token.
- `Outcome { Pass, Fail, Abstain, Escalate }`. Staleness and abstention charge nothing; the
  `plan.done_refusal_cap`-th refusal (default 3) or an escalation derives `block{on: User}`.
- A `judge` item seats 1 or 3 reader children on the cheapest model of another family (none:
  abstain); an unquoted vote abstains; Rust tallies. The jury past `plan.judge_cap` escalates.
- `Resolution { VerifiedDone, AcceptedByUser, LegacyUnverified }`. A todo with a contract or a
  stated acceptance completes only beside one, on every path (`set`, `import`, `repair`,
  `supersede` share the validator); any other on the caller's word.
- A non-error write to a path a running todo `covers` runs its `cmd` items on a copy of the
  writer's tree within 60 s and appends `contract "<label>" check: <verdict>`; it journals nothing.
- Owner: [`plan/done.rs`](../crates/runtime/src/plan/done.rs). Settled by: D194, D216, D228.

### 13.2 Worktree acceptance
A worktree todo needs a contract; it is Done only via acceptance or as `AcceptedByUser`.

- `Phase { Unsubmitted, Submitted, CandidateVerified, IntegrationPrepared, IntegrationVerified,
  IntegrationStale, Accepted, MergeFailed, Disposed }` is read from the attempt's records
  (`candidate_submitted` … `accepted`, `disposition`); `done` needs no missing record.
- `submit` commits the child's lane as the candidate; integration is prepared in a staging lane,
  never the user's checkout, and published under a generation check (re-prepared at most 2 times).
- Any other exit records `Disposition { Retained, Discarded, MergeFailed, RepossessionPending }`
  ([`acceptance.rs`](../crates/types/src/plan/acceptance.rs)); nothing merges to free a slot.
- Owner: [`plan/acceptance.rs`](../crates/runtime/src/plan/acceptance.rs). Settled by: D195.

### 13.3 Engine dispatch, finish, and the todo list
- After every op, `dispatch_ready` applies `start` as `Actor::Engine` to each ready delegated
  todo admission allows; the probe tick's `dispatch_ready_in` is the backstop.
- A plan child's end is the engine's: it stores the last answer and applies `submit`, `done`; a
  `fail` verdict fails the todo `retained`. The owner gets one `plan: accepted|refused|failed`.
- The `todo` tool keeps a session list of `custom{todo}` entries. With a plan open at depth 0 the
  list is the plan's view, re-projected by `Mirror` after each op; `todo start|done` on a plan item
  is the owner's plan op, any other change to one is `TodoError::Mirrored`.
- Owner: [`schedule.rs`](../crates/runtime/src/plan/schedule.rs). Settled by: D224, D225, D226,
  D229.

## 14. Lane
A lane is a git worktree slot, leased from a per-repository pool and handed back by move.

- A root session claims one unless `--here`, `lanes.enabled: false`, a cwd outside a repository,
  or `--headless` without `--lanes`.
- The pool is `~/.yi/lanes/<hash>/<n>`, keyed by the common git dir; `pool.lock` serializes
  claims; a slot is live iff a process holds its `.held` flock.
- A claim resets the slot to `origin/main` (else `main`, `HEAD`, or a given commit), checks out
  `yi/<session>` and locks the worktree `session:<id>`. With `lanes.slots` unset it never refuses.
- A resumed session reclaims its slot; a dead holder's clean slot with its branch in `main` is free.
- Every hand-back takes `self`: `discard` deletes the branch, `release` detaches and warms, `Drop`
  detaches. A child's lane returns through `rlm.merge_worktree` (settle, stage, publish) or
  `rlm.discard_worktree`, never for a running or plan-dispatched child (§13.2).
- `/land <title>` merges `origin/main` in, pushes, opens a pull request, and polls every 60 s for up
  to 2 h. The forge is `GitHub` (`gh`) for a `github.com` origin, else `Forgejo` (`fgj`).
- The warmer runs offline under Seatbelt, only for a lockfile hash a session already synced.
- `LaneHandle::row` alone builds the status-row text `<repo> ⎇ lane <n>`.
- Owner: [`crates/runtime/src/lane/mod.rs`](../crates/runtime/src/lane/mod.rs).

State: `ClaimBase`, `TreeState`, `SlotView { Idle, Held, Orphan }`, `Forge`. Shapes:
[`lane.rs`](../crates/types/src/lane.rs): `SlotState` at `<pool>/<n>.json`, `Landing { Unlanded,
Pushed{branch}, Open{pr, jobs, behind}, Merged{pr} }` (as `_yi/landing`), `LanesConfig{enabled,
slots, land}`. CLI `yi lanes [reap <slot>]`; slash `/lanes`, `/land`, `/discard`.

- Settled by: D119, D120, D123, D124, D130, D203, D208.

## 15. Goal and schedule
### 15.1 Goal
One objective per session, stored as `Fact::Goal` outside the transcript.

- `goal.create {objective, token_budget?, check?, check_timeout_ms?}` fails while one is open;
  `goal.update` takes `blocked`, or `complete` once discoveries drain and `check` passes.
- The host writes `BudgetLimited` once tokens used reach `token_budget` (and steers
  `budget_limit.md`), and `Blocked` after an Error stop. Nothing writes `Paused`/`UsageLimited`.
- On `AgentEnd` with an Active goal it sends `continuation.md` as a follow-up, unless one is
  pending or an Aborted stop set the latch that user input clears. Compaction drops goal prompts.
- Owner: [`goal/mod.rs`](../crates/runtime/src/goal/mod.rs). State: `GoalStatus { Active, Paused,
  Blocked, UsageLimited, BudgetLimited, Complete, Other }`
  ([`goal.rs`](../crates/types/src/goal.rs)).
- Surfaces: host requests `goal.{get,create,update}`; RPC `goal`, ACP `_yi/goal`. Settled by: D52.

### 15.2 Schedule
A `JobStore` holds jobs and claims; an in-process `Scheduler` delivers due jobs to sessions.

- One `scheduled-jobs.json` per runtime directory (the root's `rlm-<pid>/`, a child's own),
  interned by path, so the sessions on it share one store and timer.
- A claim is persisted before delivery and re-arms `next_run_at` from claim time, collapsing
  missed ticks. Recovery marks open claims `INTERRUPTED_ERROR`.
- Claimed jobs group by `Job.session_id`, serial within and concurrent across groups. The
  deliverer feeds `should_defer` whether the session is streaming, compacting, or has queued
  steer/follow-up work behind a running turn, so a heartbeat due mid-turn or mid-compaction
  defers to the next boundary instead of interleaving with it.
- Surfaces: RPC `heartbeat` and ACP `_yi/heartbeat` (the `/heartbeat` grammar, default
  `every 5m`, one per session); kernel `rlm_heartbeat.{list, create, update, delete}`.

Owner: [`schedule/mod.rs`](../crates/runtime/src/schedule/mod.rs). Shapes:
[`schedule.rs`](../crates/types/src/schedule.rs): `CronSchedule{kind: { Once, Cron, Interval }}`,
`Job`, `JobStatus { Active, Paused, Completed, Cancelled }`, `JobSource { Cron, Heartbeat,
RlmHeartbeat }`, `DeliveryMode { Steer, FollowUp }`. Settled by: D86, D246.

## 16. Advisor
A reviewer that reads a digest of the session's work log and may emit one advice per review.

- It reviews only when `models.advisor` is set; unset, it keeps the log and never speaks.
- A review is forced only on a plan change and on compaction (the summary as a `compaction:` line).
- Each review is a fresh `AgentSession`: fixed system prompt plus the cwd's `ADVISOR.md`, tools
  `advise` and `transcript{entry_id}`, and the digest since the last review; never thinking.
- A guard drops blocklisted phrases, dedupes over a 4,096 FIFO, and accepts one advice a review.
- Note and Warn arrive as `custom{advisory}` (steer while running, follow-up while idle). A Hold is
  a 1 h permission hold on the target, or a Warn with no asker.
- `/advisor promote <id>` writes `.yi/rules/<slug>-adv-N.md`, armed live; nothing else persists.

Owner: [`advisor/mod.rs`](../crates/runtime/src/advisor/mod.rs). Shapes:
[`advisor.rs`](../crates/types/src/advisor.rs). Settled by: D50, D56, D59, D80.

## 17. Surfaces
### 17.1 CLI
`yi` is the binary of `yi-cli`. Arguments are hand-parsed with `lexopt`; there is no `--help`.

| Verb | What |
|---|---|
| `yi` / `yi <words>` | TTY: bare opens `console` (`tui` with `--solo`); words open `tui` with that prompt. No TTY: bare prints a verb banner; words run `ask` |
| `ask`, `rpc`, `acp`, `serve` | One headless run (`--json` prints each event as a JSON line); a JSON-lines loop on stdio; one ACP worker on stdio; the ACP daemon (§17.2) |
| `console`, `tui` | The workspace shell (§17.4), starting a detached `serve` when none answers; the solo chat (§17.3) |
| `sessions list\|show\|rm`, `stats [id]`, `undo` | The cwd's sessions; one session file replayed for per-tool latency, failures and tokens; restore the files the last turn changed |
| `lanes [reap <slot>]`, `trust [list\|revoke]`, `gate <cmd>`, `fetch <url>` | Lane slots (§14); repository trust (§8); the permission decision for a command, exit 1 when refused (§8); one resolve through the wall (§10) |
| `plan lint\|report\|fuse reset\|repair\|accept\|resolve\|<op>`, `why <file>:<line>\|<plan>/<todo>`, `todo [list]` | Plan ops as the owner; blame to commit to todo to goal; the newest todo list (§13) |
| `memory list\|show\|forget\|import\|stats\|check`, `catalog [refresh [provider]]`, `doctor [--fix]` | Memory stores (docs/memory.md); the model catalog (§5); session invariants checked, safe ones repaired |
| `login`, `logout`, `mcp …`, `version` | Provider credentials; the MCP client (§7.6), refused unless `mcp.enabled`; `yi <version>` |

- The default permission mode is `auto`; `--confirm` selects `ask`, `--yolo` selects `yolo` (§8).
- Exit codes: 0 ok; 1 error; 2 usage, bad flag, bad config or refused build; 3 an answer failing
  `--schema`. Under `--json` an agent failure is in-band and exits 0. Errors print `error: …`.
- `~/.yi/config.json` is the only config file, parsed once; every struct is `deny_unknown_fields`
  and `migrate` drops keys an older build read, reporting each. Keys: `model`, `thinking`,
  `models{primary,summarizer,advisor,autoReview}`, `bash{autoBackgroundMs}`, `plans{dir}`,
  `plan{staleReminderTurns}`, `mcp{enabled,tokenStore}`, `kernel{prewarm}`, `console{autoSide}`,
  `edit{freeformGrammar}`, `keys{<action>:<key>}`, `tui{pace}`, `lanes{enabled,slots,land}`,
  `catalog{enabled,refreshHours}`, `telemetry{enabled}`, `routing`, `rlm{maxDepth}`.
- The default cargo feature `tui` gates `yi-tui` and `yi-console`; without it both verbs exit 2.
- Owner: [`main.rs`](../crates/cli/src/main.rs); config:
  [`config.rs`](../crates/types/src/config.rs)

### 17.2 ACP and daemon
`yi-acp` speaks a hand-rolled ACP v2 subset over JSON-RPC 2.0, shapes in `yi-types::acp`.
`initialize` below version 2 fails −32602, an unknown method −32601, bad JSON −32700.

| Method | Side | What |
|---|---|---|
| `initialize`, `session/new`, `session/resume{replayFrom?}`, `session/list` | both | The daemon routes new/resume by cwd and lists from its ledger without a cwd |
| `session/prompt`, `session/cancel`, `session/close`, `session/delete`, `session/set_config_option` | worker | A busy session queues a prompt as a follow-up (§4.3); config ids `mode`, `model`, `thought_level` |
| `_yi/kernel_execute`, `_yi/kernel_cancel`, `_yi/tracked`, `_yi/slash` | worker | Kernel code (§9); tracked paths (≤ 64); a session slash verb |
| `_yi/heartbeat`, `_yi/goal`, `_yi/steer`, `_yi/rewind`, `_yi/todo`, `_yi/plan`, `_yi/child_answer`, `_yi/child_replay`, `_yi/child_abort` | worker | Solo verbs over the wire (§4, §11, §13, §15) |
| `_yi/shutdown`, `_yi/seen` | daemon | Stop; clear a session's unseen count |
| `session/request_permission` | worker → client | Options `allow_once`, `allow_always`, `reject_once` |

`session/update` carries the standard kinds (`agent_message_chunk`, `agent_thought_chunk`,
`tool_call_update`, `state_update`, `usage_update`, `terminal_update`), passes unknown kinds through
as `Extension`, and adds: `_yi/event` (every `AgentEvent` verbatim, per-session `seq`),
`_yi/event_gap` (a broadcast lag), `_yi/replay` (a branch verbatim, 512 entries per frame),
`_yi/config`, `_yi/goal`, `_yi/todo`, `_yi/workdir{cwd,lane}`, `_yi/landing`,
`_yi/subagent_update`, `_yi/heartbeat_changed`, `_yi/compaction` (replay only), `_yi/<custom_type>`.

`yi serve` is a supervisor on `~/.yi/daemon.sock` (mode 0600): one `yi acp --cwd <root>` worker
per root, ACP v2 on both hops, request ids rewritten to `sup_N`; a second instance exits 0. The
ledger `~/.yi/daemon.ledger.json` is rewritten whole by rename and reloads with every state idle.

- The daemon owns no session; workers do, and a worker's exit fails its pending requests.
- A row's activity is its last state transition, never a chunk; `unseen` counts only while detached.
- A worker request to a detached session parks, at most 8; past that the worker gets an error.
- Output fans out to every attached client and the first answer wins; a client at 4096 queued
  frames is dropped.
- Owner: [`lib.rs`](../crates/acp/src/lib.rs), [`daemon.rs`](../crates/acp/src/daemon.rs)
- State: `Input{Client, ClientLine, ClientClosed, WorkerLine, WorkerReply, WorkerClosed, Shutdown}`,
  `SessionEntry{root, attached, unseen, last_state, last_event_ms, name, provisional}` (daemon.rs)
- Shapes: [`crates/types/src/acp.rs`](../crates/types/src/acp.rs) (`DaemonLedger`)
- Settled by: D1, D4, D40, D95, D113, D118

### 17.3 TUI
`yi-tui` is the solo chat: an inline viewport on the normal screen above native scrollback.

- The UI thread is synchronous; the runtime runs on its own thread; intents return as `Command`s.
- Finished cells commit above the viewport once, through scroll regions, in one synchronized
  update per frame. Streaming text commits by byte cursor over stable slices; a thought commits
  before its prose; a reveal cursor paces the painted tail.
- Resize and mode change rebuild scrollback from the transcript; text wraps at width less 2; a
  frame draws only on change.
- A tool call is a card: an outcome-coloured `│` rail, per-tool hue, right-aligned chips, 3 result
  rows then `… N more lines` when collapsed.
- Bottom stack: live tail, working line or orb, HUD (goal, plan progress, numbered todo block
  `Todos done/total`; `ctrl+t` hides it), composer or bottom view, status row (model, effort,
  lane row, landing, cost, `used / window`, session name).
- The palette is fixed: tier from `COLORTERM`/`TERM`, light or dark from `COLORFGBG` (default
  dark). The orb is [`yi-orb`](../crates/orb/src/lib.rs), painted over kitty graphics.
- `yi tui --headless --keys <script> --frames <dir> [--record] [--snap]` drives the real loop on
  an in-memory screen (steps `key`, `type`, `type-ms`, `wait`, `wait-idle`, `wait-frame`, `quit`);
  it implies `--here` unless `--lanes`, and the drive flags are refused without `--headless`.
- State: `Cell { User, Assistant, Thought, Tool, Explored, Task, Advisory, Notice, Footer, Rule,
  Divider }`.
- Owner: [`app.rs`](../crates/tui/src/app.rs), [`drive.rs`](../crates/tui/src/drive.rs)
- Settled by: D45, D47, D48, D73, D107, D126, D131, D136, D198, D199, D202, D204, D208

### 17.4 Console
`yi-console` is the workspace shell: an ACP client of the daemon (§17.2) on the alternate screen
with mouse capture. It depends on `yi-types` and `yi-tui` only.

- The protocol reducer runs synchronously in the UI loop; IO threads only move frames.
- A session pane runs `yi-tui`'s chat reducer, fed from `_yi/event` through a port.
- A drag copies the words under it, without Yi's rails and gutters, over OSC 52.
- A status change notifies over OSC 9 or kitty OSC 99 once it holds for 1 s.
- State: `PaneContent{Session, Markdown, Diff, Notebook, SessionDiff, Editor}`,
  `SessionStatus{Blocked, Working, DoneUnseen, Idle, Unknown}`, `SidebarMode{Rail, Full}`,
  `Link{Connecting, Connected, Disconnected}`, `Mode{Normal, Prefix, Navigator, Keys}`
- Owner: [`lib.rs`](../crates/console/src/lib.rs), [`model.rs`](../crates/console/src/model.rs)
- Settled by: D95, D96, D112, D113, D141, D201

## 18. Dependencies and size

- Owner: [`Cargo.toml`](../Cargo.toml), [`deny.toml`](../deny.toml),
  [`check_manifests.py`](../scripts/guardrails/check_manifests.py). Settled by: D31, D48, D69, D71,
  D74.

### 18.1 Rules
- Every dependency, path crates included, is declared once in the root `[workspace.dependencies]`;
  a crate's `[dependencies]` entry is `{ workspace = true }` (`check_manifests.py`).
- Every external entry sets `default-features = false` and names its features, except
  `thiserror`, `lexopt` and `vt100`.
- A crate on the §18.5 list enters the graph only through a `wrappers` exception in `deny.toml`
  scoped to that crate.
- `cargo deny check` enforces the license allowlist, the §18.5 bans, `multiple-versions = "deny"`
  (7 skips), yanked advisories, and no unknown registry or git source, over four targets
  (aarch64/x86_64 darwin and linux-gnu) with dev-dependencies excluded. `cargo machete crates`
  fails on a declared dependency no code uses.
- Adding a crate edits §18.3, `deny.toml` where a ban or wrapper applies, and
  `docs/size-ledger.md` in one commit.

### 18.2 Profiles
`release` is cargo's default (no `[profile.release]`), unwinding intact. `dist` ships and is
what §18.6 measures: `inherits = "release"`, `opt-level = "z"` (also for
`package."*"`), `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`,
`debug = false`, `incremental = false`. `scripts/build_dist.sh` builds it.

### 18.3 Allowed dependencies
| Crate | Features | Used by | Reason | Alternative considered |
|---|---|---|---|---|
| `serde` | `derive`, `std` | types, session | every wire and disk shape (§20) | hand-rolled JSON: compat correctness matters more |
| `serde_json` | `std`, `preserve_order` | all but orb, permission | JSON with key order kept, so a session file round-trips byte for byte | same |
| `tokio` | `rt`, `sync`, `time`, `macros`, `process`, `io-util`, `net`; no `rt-multi-thread` | ai, kernel, loop, runtime, acp, tui, cli | provider streams, kernel sockets, scheduler timers | `smol`: `zeromq` is tokio-shaped |
| `ureq` | `tls`, `native-certs` | ai, oauth, mcp-cli, kernel | blocking HTTP and SSE to providers, OAuth, MCP HTTP, the uv download | `reqwest` (hyper stack), `native-tls` (openssl on Linux) |
| `zeromq` | `tokio-runtime`, `tcp-transport` | kernel | Jupyter DEALER/SUB channels | `zmq`: libzmq FFI |
| `hmac` | — | kernel | Jupyter message signing | — |
| `sha2` | — | types, oauth, permission, runtime, kernel | message signing, permission digests, PKCE, plan digests | — |
| `xxhash-rust` | `xxh32` | tools | hashline tags | — |
| `globset` | — | permission, tools, runtime | permission patterns, file tools | `glob`: no brace sets |
| `regex` | `std`, `perf`, `unicode-case` | tools | the `grep` tool; full Unicode tables stay out | — |
| `lexopt` | — | cli | argument parsing | `clap`: size and startup |
| `thiserror` | — | types, oauth, session, permission, tools, mcp-cli, kernel, runtime | typed errors at crate boundaries (§19) | `anyhow` (banned) |
| `miniz_oxide` | `with-alloc` | orb, ai, kernel, tui | zlib for the kitty orb's `o=z` frames; inflates the build-time-packed model catalog, Python runtime and logos, and the uv archive | `flate2`: wraps this crate or `libz-sys`; `t=t` temp-file transmission |
| `ratatui` | `crossterm`, `scrolling-regions` | tui, console | terminal rendering | — |
| `tui-textarea` | `crossterm` | tui, console | the composer | — |
| `pulldown-cmark` | — | tui | Markdown rendering | — |
| `syntect` | `parsing`, `default-syntaxes`, `regex-fancy` | tui | syntax highlighting on the pure-Rust regex engine | `onig` (C engine, banned) |
| `unicode-width` | — | tui | terminal cell width | — |

Dev: `vt100`, `insta` (tui), `proptest` (types, runtime), `rmcp` with `server`, `transport-io`,
`macros` (mcp-cli's reference server). Build: `miniz_oxide` (ai, kernel, tui) packs embedded assets.

### 18.4 Features
Only `yi-cli` declares features: `default = ["tui"]`, `tui = ["dep:yi-tui", "dep:yi-console"]`.
`check_manifests.py` fails on a feature outside its per-crate allowlist.

### 18.5 Banned
`deny.toml` `[bans].deny`: `reqwest`, `hyper`, `openssl-sys`, `native-tls`, `git2`,
`libgit2-sys`, `gix`, `clap`, `anyhow`, `once_cell`, `rand`, `lazy_static`, `textwrap`, `toml`,
`onig`, `onig_sys`, `tracing-subscriber`. Wrapper exceptions: `once_cell` through `rustls`,
`ureq`, `zeromq`, `syntect`; `rand` through `zeromq`.

### 18.6 Budgets
| Budget | Limit | Measured as | Gate and baseline |
|---|---|---|---|
| direct deps | 20 | distinct non-`yi-` names across every crate's `[dependencies]`, optional ones included | `check_deps_budget.py`, `baselines/deps_budget.json` |
| transitive deps | 167 | distinct non-`yi-` names in `cargo tree -e normal --workspace` | same |
| dist binary | 7,340,032 bytes | size of `$CARGO_TARGET_DIR/dist/yi` (default `target/`) | `check_binary_size.py`, `baselines/binary_size_budget.json` |
| `yi --version` | 5.0 ms | minimum of 50 `hyperfine -N` runs after 10 warmups on the dist binary | `check_startup.py`, `baselines/startup_ms_budget.json` |

The binary and startup gates run only after a local dist build; a failed build fails both. Under
`--fast` and when `CI` is set they are skipped by a printed line (D68, D70, D91).

## 19. Code style (HAR)
The enforced rules for Rust in `crates/`; production lines precede a file's first `#[cfg(test)]`.

- Settled by: D49, D55, D109.

| Rule | Gate |
|---|---|
| `#![forbid(unsafe_code)]` on every crate root | rustc; no script checks the attribute is present |
| clippy denies `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `await_holding_lock`, `await_holding_invalid_type` (`std::sync::MutexGuard`, `RwLockReadGuard`, `RwLockWriteGuard`), `disallowed_methods`; `unwrap`/`expect` are allowed in tests | `[workspace.lints.clippy]`, [`clippy.toml`](../clippy.toml); `just lint` runs clippy `-D warnings` and `fmt --check` |
| disallowed methods `std::process::Command::new`, `std::time::SystemTime::now`, `String::truncate`; each call site carries a scoped `allow` | same |
| panic budget 0: no `.unwrap()`, `.expect(`, `panic!(`, `todo!(`, `unimplemented!(` in production lines | `check_panic.py`, `panic_budget.json` |
| `#![deny(clippy::string_slice)]` on every crate root outside the shrink-only pending list (`ai`, `context`, `kernel`, `mcp-cli`, `runtime`, `tools`) | `check_manifests.py`, `string_slice_pending.json` |
| fallible crate boundaries return `thiserror` enums; `anyhow` is banned | `cargo deny` (§18.5) |
| ids and units that cross a module get a newtype (`Tokens`, `Bytes`, `JobId`, `TreeId`, `PrNumber`, console `SessionId`) | review; no gate |
| a comment run is ≤ 2 lines (a line-1 license header is exempt) and the over-cap count may not grow; comment volume outside `crates/types` may not grow | `check_comments.py`, `comment_budget.json` |
| a comment opening with `Word:` uses a closed grant, `Incident:` or `Invariant:`; a comment under 8 words once its decision ids are stripped is rejected | `check_comments.py` |
| `rustdoc::broken_intra_doc_links = "deny"`; `private_intra_doc_links` is allowed (D55) | `cargo doc --workspace --no-deps --document-private-items` in `check_guardrails.sh` |
| a dist build aborts on panic | the §18.2 `dist` profile |

## 20. Schema stability
- Every `Deserialize` derive lives in `yi-types`; outside it only
  [`query.rs`](../crates/session/src/query.rs) in yi-session derives `Serialize`, for query output.
- A shape changes only through a reviewed diff of `baselines/schemas.lock`, which maps
  `<file stem>::<Name>` for every `pub struct` and `pub enum` in `crates/types/src` to its source,
  comments stripped and whitespace collapsed. `check_schemas_lock.py` fails on a removed, added or
  changed shape until `--update`, which lands in its own commit (§21).
- Unknown data survives where the shape allows it: structs carry `#[serde(flatten)]` maps
  (38 in `crates/types/src`); open wire enums end in `#[serde(untagged)] Other(String)` (e.g.
  `AcpStopReason`, `SpanKind`) or a `#[serde(other)]` unit variant.
- Config structs in [`config.rs`](../crates/types/src/config.rs) and `LanesConfig` in
  [`lane.rs`](../crates/types/src/lane.rs) set `deny_unknown_fields`: an unknown key is an error.
- Golden fixtures in [`crates/types/tests/fixtures`](../crates/types/tests/fixtures) are added,
  never deleted; no gate checks deletion.
- Owner: [`crates/types/src`](../crates/types/src),
  [`check_schemas_lock.py`](../scripts/guardrails/check_schemas_lock.py)

Versions: the session file is §4.1 (`Entry` is internally tagged, no catch-all); permission state
`SessionPermissionState.version`, default 2; kernel venv `BOOTSTRAP_SCHEMA = 1` (§9.1); ACP
`protocolVersion` at `initialize` (§17.2).

## 21. Guardrails
`scripts/guardrails/check_guardrails.sh` runs every gate below in parallel and prints the reports
in launch order. `just check` is `lint`, `guardrails` and `test`; the pre-commit hook runs
`--fast`, which skips the dist build, `binary_size`, `startup`, `growth`, `cargo doc`, `machete`
and `deny`. Baselines live in `scripts/guardrails/baselines/`; a gate's `--update` records growth.

| Gate | Enforces | Baseline |
|---|---|---|
| `check_manifests` | folder `x` is crate `yi-x`; workspace version, edition, license, rust-version, lints; deps `{ workspace = true }`; feature allowlist; string_slice roots (§19) | `string_slice_pending.json` |
| `check_boundaries` | each crate's `yi-*` deps are in its allowlist; unknown crate or stale entry fails | `boundaries.toml` |
| `check_filenames`, `check_glob_reexport`, `check_orphans` | no `part_N.rs` or `_NN.rs` source file; no `pub use …::*` or `use super::*` in production lines; no write-only `pub` field, no baseline without a reader (D109) | — |
| `check_commit_style` | subjects on `HEAD --not origin/main`: one imperative line, ≤ 72 chars, no assistant trailers; a baseline edit never shares a commit with code (merge commits exempt) | — |
| `check_panic`, `check_comments`, `check_schemas_lock` | §19, §20 | `panic_budget.json`, `comment_budget.json`, `schemas.lock` |
| `check_deps_budget`, `check_binary_size`, `check_startup` | §18.6 | `deps_budget.json`, `binary_size_budget.json`, `startup_ms_budget.json` |
| `check_file_size`, `check_fn_size`, `check_crate_size` | a `src/` file ≤ 1,200 lines; a function ≤ 150 lines, counted by braces; per-crate `src/` line ceilings | `crate_size_budget.json` |
| `check_duplication` | no 15-line normalized window repeated across production `.rs` | — |
| `check_test_size`, `check_test_tiers` | total `crates/*/tests` lines; every `#[ignore]` carries exactly the `just journeys` tier-2 reason | `test_size_budget.json` |
| `check_env_surface`, `check_blob_size` | every `YI_*` name in `src/` is declared, at most 40 declared; no tracked file > 512,000 bytes outside the allowlist | `env_vars.json`, `blob_allowlist.txt` |
| `check_public_surface` | after the mirror's exclusions and substitutions no file matches a deny pattern (D172) | `scripts/mirror/{exclude,replace,deny}.txt` |
| `check_request_budget` | system-prompt and tool-table bytes; tool-surface hashes (D188) | `request_budget.json`, `tool_surface.json` |
| `check_behavior` | faux-cassette behavior cases: a locked pass never fails (D76) | `behavior_baseline.json` |
| `check_prompt_examples` | Python in prompts and skills awaits every `yi` coroutine call | — |
| `check_growth` | net `src/` growth over 150 lines needs a `growth +N:` changelog memo, over 2,000 a D-row cite; full run only | `src_loc.json` |
| `check_pr_metadata` | PR title as `check_commit_style`; a feature or over-band PR names an open, sized issue (D106); CI only | — |

Also run: `codespell`, the `python/yi_runtime` unittests, `evals/selftest.py`, each script's
`--selfcheck`, `cargo doc` (§19), `cargo machete` and `cargo deny` (§18.1); a missing tool fails
its gate. Settled by: D68, D70, D76, D91, D106, D109, D172, D188.

