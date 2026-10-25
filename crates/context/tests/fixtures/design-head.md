# Yi design

Yi is a coding agent written in Rust: one `yi` binary, a persistent Python kernel beside it, and
sessions stored in Pi's v4 session JSONL format. This document states what the code does. §3
names the primitives; each primitive section lists its invariants, owner, state, shapes and the
D-rows in [ARCHITECTURE.md](ARCHITECTURE.md) that settle it.

## 1. Scope
### 1.1 What Yi is
| Topic | Decision | Settled by |
|---|---|---|
| Language | Rust workspace of 17 crates (§2); every dependency declared once in `[workspace.dependencies]` (§18) | D31 |
| Session file | Pi's v4 session JSONL format: a header, then one `Mutation` per line; any other version is rejected (§4.1) | D32 |
| Pi RPC | `yi rpc` serves Pi's RPC JSONL command names plus Yi's own (§17.1) | |
| Providers | Four wire APIs (Anthropic messages, OpenAI completions, OpenAI responses, Cursor agent) plus `faux`; a compiled-in model catalog (§5) | |
| Login | `yi login` / `yi logout` in `yi-oauth`; no provider identity is compiled in except `cursor` | D191 |
| MCP | A one-shot CLI (`yi mcp`), compiled into every build, refused unless `mcp.enabled` is true; the agent reaches it through `bash` and the kernel, never a registered tool (§7.6) | D36, D71 |
| Python | A Jupyter kernel over ZeroMQ, its package embedded in the binary, its toolchain a pinned, verified uv (§9) | D156, D238 |
| Permission | Modes and rules, a per-segment command classifier, a catastrophic denylist, an optional model reviewer, Seatbelt containment on macOS (§8) | D81, D205 |
| Worktrees and node | Resource admission: a root session claims a git worktree slot unless `--here`, `lanes.enabled: false` or no repository (§14); one machine is a node card, `~/.yi/node.json`, and every live kernel on it takes one of its slots or shares its family's (§9); a child may run its bash in a local container (§11) | D119, D203, D285, D286 |
| ACP | v2 only; a lower `protocolVersion` gets a version-mismatch error; the wire is hand-rolled and held to a vendored upstream schema tag (§17.2) | D1, D40, D328 |
| Workspace | `yi serve` owns sessions; `yi console` is an ACP client over its socket; bare `yi` on a terminal opens the console, `--solo` the TUI (§17) | D95, D118 |

### 1.2 Not in scope
A top-level feature enters only when one leaves, and this list changes in the same commit.
A resident MCP client or MCP as a registered tool · ACP v1 · automatic skill creation,
refinement or self-extension · embeddings and semantic search · resident LSP or DAP servers · a
JavaScript runtime or Node sidecar · embedding `yi-runtime` through N-API or WASM · document
converters compiled into the binary · Windows · dynamically loaded plugins (extensions are
in-process Rust, §6; a `yi plugin` is a pinned external program whose reader is compiled in,
§17.1) · a GUI · browser automation as an agent tool, computer use, email, or any capability that
is not a coding agent (the console's browser pane shows the user a page and gives the model nothing) ·
server-side terminal-frame streaming or blit
encoding in the console (a terminal pane's PTY is the console's own, §17.4) · the console's
Tape pane, a session's ledger over wall time (left when the terminal pane entered) · a keys overlay beside the settings page (the keys are its tab) · a scheduled job that puts its own text in front of the model or runs saved code
(a tick creates a todo, §15.2) · a polling probe ladder (an `External` probe is an `exec`
channel wait, D287) · channel consumer groups and a channel home on another machine · placement on another machine, a VM or a
cloud, and a kernel or agent loop inside a container (a container child's kernel stays on the
host, D286).

## 2. Crates and dependency direction
A folder `crates/x/` is the crate `yi-x` (`scripts/guardrails/check_manifests.py`). The internal
dependency graph is the allowlist in `scripts/guardrails/boundaries.toml`; `check_boundaries.py`
fails on an undeclared edge, an unlisted crate or a stale entry. Size ceilings are in §18.6.

| Crate | Owns | Internal deps |
|---|---|---|
| `yi-types` | Serialized shapes: messages, events, entries, wire, config, plan, mail, lane, url | none |
| `yi-loop` | `run_loop`, `LoopConfig`, `AgentTool`, interrupt signal, tool-name repair | yi-types |
| `yi-ai` | Provider streams, model catalog, retry, SSE | yi-types, yi-oauth |
| `yi-oauth` | PKCE, loopback callback, token store, `login`/`logout` | yi-types |
| `yi-orb` | Orb geometry and kitty-graphics painter | none |
| `yi-session` | Session store: JSONL and in-memory repos, ids, queries | yi-types |
| `yi-context` | Token accounting, compaction cut point, context assembly | yi-types, yi-tools |
| `yi-permission` | Modes, rules, command classifier, catastrophic denylist, review ledger | yi-types |
| `yi-tools` | `Tool` trait, builtin tools, hashline edits, bash jobs, sandbox, checkpoints | yi-types, yi-permission |
| `yi-mcp-cli` | The `yi mcp` one-shot JSON-RPC client | yi-oauth, yi-types |
| `yi-kernel` | Jupyter client: connection file, ZMQ framing, HMAC, snapshot, uv install | yi-types |
| `yi-runtime` | `AgentSession` and everything that composes the crates above | yi-types, yi-loop, yi-ai, yi-session, yi-context, yi-permission, yi-tools, yi-kernel, yi-browser |
| `yi-acp` | ACP v2 server and the `yi serve` daemon | yi-types, yi-runtime |
| `yi-tui` | The ratatui shell | yi-types, yi-runtime, yi-orb |
| `yi-console` | The multi-pane workspace, an ACP client of the daemon | yi-types, yi-tui, yi-browser |
| `yi-browser` | Chrome discovery, the `yi __fds` shim, the CDP pipe and its router | yi-types |
| `yi-cli` | The `yi` binary, composition root, `yi rpc`, `yi ask` | yi-types, yi-runtime, yi-acp, yi-tui, yi-console, yi-mcp-cli, yi-browser |

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
- `yi-browser` starts the user's Chrome, through `yi __fds`, and runs `yi plugin add chrome`
  when there is none, for the console's browser pane; `yi-runtime` depends on it only to retain
  crashed browser profiles.
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
| Lease | The tokens and deadline a child draws from its parent's own, never more | §11 |
| Wall | A hereditary deny set over paths and URLs that only narrows down the family | §11 |
| Envelope | A message written to the receiver's session store before delivery | §12 |
| Todo | One unit of work, the same type in a session list and in a plan | §13, D282 |
| Plan and contract | A hash-chained op journal over todos; a done-predicate over a frozen attempt | §13 |
| Lane | A leased git-worktree slot with a moving hand-back | §14 |
| Node | One machine's card of kernel slots, which every live kernel is admitted against | §9, D285 |
| Schedule | A clock subscription whose tick creates a todo | §15.2, D283 |
| Channel | A durable JSONL buffer that adapters append to and a blocked todo waits on | §15.2, D287 |

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
| Heartbeat | Schedule + Todo + Session run queue (steer or follow-up) | D283 |
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
- Per request: `maybe_compact` → `transform_context` → `convert_to_llm` → `StreamFn::stream`;
  once the interrupt has fired neither `maybe_compact` nor `StreamFn::stream` runs (§4.5).
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

- Owner: [`crates/loop/src/`](../crates/loop/src/); Settled by: D145, D163, D175, D178, D179, D197, D245
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
- `convert_to_llm` reads bare only its requester's words: a user message attributed `user` or
  `task`, or one dated before attribution existed. Every other user-role message (`host(source)`,
  undated `unproven`), every `custom` kind and both summaries arrive as
  `<yi_internal_context source="…">`; compaction drops the kinds `heartbeat_prompt, advisory,
  goal_prompt, ledger_prompt, plan_dispatch, reminder, fragment, todo_intercept` (D342).
- Reclaim runs inside the run's `convert_to_llm`, before compaction is ever due: a tool result
  at least 1,200 chars long and 5 requests old (never `ask_user`, `plan`, `todo`, or the newest
  `read` of a file the session edited, by the `[path#TAG]` headers the tools print) is cut once the tokens dropped × cache-read price × requests so far beat the
  tokens after the first cut × (write − read price), or the cache lapsed; at least 2,000 tokens.
  A `custom{reclaim}` entry names the cut results, which render from then on as one
  `[reclaimed: … get it: read("history://self/<entry>")]` line; the stored results never change.
- `annotate` writes `custom{annotation}`: `pin` keeps the newest result whose call contains
  `call` out of every cut, `discard` waives its age and size floors, and a `finding` (note ≤ 280
  chars) is listed whole under `[Findings]` in every compaction view.
- Due when scheduled (`/compact`, `compact.run`) or when the whole request would leave less than
  16,384 tokens of the window for the summary request (last usage + bytes/3 after it; D338), or
  would pass `compaction.at` tokens when that comes first (a root session's ceiling, raised with a
  warning to `keep_recent` plus twice the reserve, which a compaction can get under). A
  usage the latest compaction kept in its tail is stale, and with none the messages count alone
  at bytes/3.
  The cut keeps 20,000 recent tokens, never at a tool result; up to 64,000 tokens of summarized
  user text join the tail.
- The summarizer replays the loop's request (system prompt, tools, effort of its last request) and
  converted messages plus a trailing directive; `models.summarizer` on another model sends no tools
  or thinking. A blank or failed reply retries once without the first quarter, cold; a cold
  attempt has no tools, so it sends the history as one labeled text (`serialize_conversation`,
  tool output indented under its label); with another summarizer model every attempt is cold.
  If that fails too the history stays, one `[compaction failed: …]` notice is queued, and the
  run asks no more (the next prompt retries, `/compact` instructions kept). A history at or past
  the window gets two cold attempts instead, never opening on a tool result; if both fail, the
  oldest messages leave the view until the rest fits at bytes/3 beside the system prompt, tools
  and summary. The first user message after the summary, the work's opening message (the run's
  prompt or a mid-run follow-up) and the earlier summary's words stay; the cut is never a tool
  result. The summary keeps the view, and one `[compaction elided N earlier messages …]`
  text ends it, marks the gap and tells the user (D337). The summary leads with `<yi_compact_view>`.
- A compaction whose entry fails to write is not applied: history and window stay as they were, a
  `[compaction not saved: …]` notice reaches the model, and auto-compaction pauses until an
  explicit `/compact` succeeds.
- Owner: [`crates/runtime/src/compaction.rs`](../crates/runtime/src/compaction.rs),
  [`crates/context/src/`](../crates/context/src/)
- Shapes: `Entry::Compaction { summary, retained_tail, tokens_before, details?, usage? }`,
  [`CompactionDetails`](../crates/types/src/compaction.rs) `{ readFiles, modifiedFiles, window?,
  extra }`. Settled by: D115, D337, D338

### 4.5 Interrupt
- `abort` fires `InterruptSignal` (sets `fired`, bumps `epoch`, wakes waiters); a run clears
  `fired` only if the epoch is the one read at admission. A streaming request ends with its
  partial as `Aborted`; every call not yet run is answered `ToolErrorKind::Aborted` with its
  start and end events; running tools see their cancel flag. A fired interrupt sends no further
  request, a compaction summarizer's included: the run settles as an empty `Aborted` turn.
- `cut` is a separate flag, set by the loop and read by provider pumps between SSE events.
- Owner: [`crates/loop/src/interrupt.rs`](../crates/loop/src/interrupt.rs); Settled by: D163, D245

## 5. Provider
yi-ai streams one assistant message per request as `AssistantMessageEvent`s on a 256-slot channel.
- Dispatch by `model.api`: `anthropic-messages` (`anthropic`), `openai-responses` (`openai`,
  `openai-codex`), `openai-completions` (`openrouter`, `google`), `cursor-agent` (`cursor`);
  any other api is faux. Catalog:
  deflated bundle overlaid by `~/.yi/catalog/<provider>.json`. Model: `--model`, else
  `models.primary`, else `model`; none is built in.
- Credentials: env key (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `LAYA_API_KEY`,
  `GEMINI_API_KEY`), else the yi-oauth store; OAuth refreshes before use, falling back to disk.
- Retry before the response body only: 408, 409, 429, 5xx, transport errors; 3 attempts, 300 s
  wall; `retry-after(-ms)` if ≤ 60 s, else 0.5 s·2ⁿ ≤ 8 s, no jitter; read timeout 60 s.
- `leak::recover_in` in both mappers' `finish` turns each well-formed text `<tool_call>` block
  into a `ToolCall` (id `leak-N`) and `Stop` into `ToolUse`.
- Stable cached prefix: `read`'s description fixed per session, content-hashed fence ids, a
  minute-cached `time:`; adapters set `cache_control`, `prompt_cache_key`, cache retention.
- yi-oauth: PKCE S256 from `/dev/urandom`, a `127.0.0.1` loopback, `login`/`logout`; tokens in
  `~/.yi/providers/tokens/<provider>.json` (0600, tmp+rename), refreshed 30 s before expiry under
  a per-provider flock; profiles are the user's `~/.yi/oauth/<provider>.json`. Yi ships no client
  id except `cursor`: `yi login cursor` polls `api2.cursor.sh` and a turn posts Connect protobuf
  over HTTP/2 to `agentn.us.api5.cursor.sh`'s agent `Run` method. Text frames are sent on as they
  arrive, a native shell frame runs bash, and the stream closes when the session aborts. The model
  list is that account's `AvailableModels`, cached under `~/.yi/catalog`; a typed id still runs
  when the file is absent. A profile file at the same id still wins.
- Owner: [`crates/runtime/src/provider.rs`](../crates/runtime/src/provider.rs),
  [`crates/ai/src/`](../crates/ai/src/), [`crates/oauth/src/`](../crates/oauth/src/)
- State: `models { primary, summarizer, advisor, autoReview, classifier }` in `~/.yi/config.json`
  ([`config.rs`](../crates/types/src/config.rs)), advisor, autoReview and classifier off until named;
  [`RetryPolicy`](../crates/ai/src/retry.rs). Shapes: [`model.rs`](../crates/types/src/model.rs).
- Settled by: D34, D57, D128, D191, D197, D231

## 6. Prompt
The system prompt is a table of ranked slots plus a yard of fenced external text, owned by an
extension `Host` whose synchronous extensions turn session events into effects.

- `PromptState::assemble` joins three blocks with `SYSTEM_BLOCK_SEPARATOR`: ranks ≤ `Doctrine`,
  ranks > `Doctrine`, the yard. `ext::install` attaches identity, doctrine, the permission-mode
  fragment, user system text and schema instruction (text compiled in from
  `crates/runtime/src/prompts/`); a headless ask (`yi ask` or bare prompt words) with no terminal on stdin also attaches
  `unattended` (Mode rank): nobody answers, hidden checks grade the task text, a check per
  requirement, one verifier cell before stopping. Then it registers `project-resources` (skills catalog), `pack`
  (`lang-rust`, `~/.yi/extensions/*.json`), `orchestrate`, `plugins` (the card of each plugin found,
  §17.1), `route-telemetry`, `memory`.
- Yard text never enters a slot. It renders as `<<<yi-external <id> source="…" trust="…">>>`,
  `<id>` = first 16 hex of the text's content hash; `<<<` is escaped, control chars stripped.
  Project instruction files (`AGENTS.md`, `CLAUDE.md`; 48 KiB each, a pair equal but for HTML
  comment lines rides once, the granted one if either is; a file whose bytes are `HEAD`'s blob is
  granted) and project skills render here. Project
  packs load only when `~/.yi/trust.json` (`yi trust`) grants their content hash.
- The table persists as `custom{ext_state}` on change and is restored on resume.
- The environment block is a `host_user` message appended per request by `transform_context`,
  never stored: `cwd`, `files` (20), `landing`, `todos`, `time` (per minute), `deadline`,
  `platform … · host <line>`, `model`, `context`, `kernel`, children. `host` comes from
  `host::probe_at` (DMI, cpuinfo, container markers, SSH, WSL), probed once; SSH is re-read.
- Reminders fire on evidence: `orchestrate` attaches silently at turn end only after an edit and
  more calls than its lever; `edit_before_read`, `files_matched`, `failed_check_after_edit` remind.
  `RuleEngine` reads `~/.yi/rules` and `.yi/rules` (none built in). A `yi://skills/` hint answers
  only a message the user typed, placed right behind it: ≤ 2 a message, a typed `$name` always.
  With `models.classifier` set, a local sidecar is also asked which triggered skill the message
  calls for (`classify` entry); it points only at or above `classifier.threshold`.
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
- Settled by: D138, D139, D187, D188, D196, D209, D219, D220, D231, D289.

## 7. Tool
A `yi_tools::Tool` adapted to the loop's `AgentTool`; the set is fixed at session build.

### 7.1 Table and trait
| Read | Write | Exec | Ledger |
|---|---|---|---|
| `read`, `grep`, `get_context`, `ask_user`, `annotate` | `edit`, `write` | `bash`, `ipython` | `plan`, `todo` |

- `builtin_tools_with` builds `read edit write grep bash get_context`; `session_tools` adds exec
  tools from `~/.yi/tools` (`--schema`, JSON on stdin, exit ≠ 0 is an error); wiring pushes
  `ipython`, `annotate`, `plan`, `todo`, and `ask_user`. `check_request_budget.py` prices system block plus tool
  table against a shrink-only baseline and locks a sha256 per tool in `tool_surface.json`.
- Deferred guides: the table is byte-constant for a session, so the documentation is deferred,
  never the tool. `edit`, `plan`, `todo` and `ask_user` carry a summary, their full schema
  (`plan` without field prose below the top level) and `read("yi://tools/<name>")`, which serves
  the whole guide; `read`, `grep`, `bash`, `write`, `get_context` and `ipython` stay whole. An
  edit refusal names the address. There is no dispatcher tool.
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
past it: strip ANSI, compress, filter (cargo; grep/rg/ag 60 lines, 3 a file; else head 80/tail
40), kept only if shorter. Runs of passing-test lines become one counted row, and a line naming an
error or failure in the cut middle is kept with 3 lines either side, 60 at most (`SIGNAL_ROWS`).
With `models.selector` named, the lines of a cut output's full text (at most 4,000) that the cut
does not show, never a passing-test line (the reducer's `is_pass`), go to that model in concurrent parts of 300 (`PART_LINES`), numbered as in the full output, and each answers 1-based line ranges
only; within 5 s the unshown lines it names follow the cut verbatim under a row naming it, else a
row says it gave none (`side:select` books each part's cost); a part holds to 200 / parts lines and one
that gives none is named in the row; nothing left out asks nothing. Lossy output is tee'd, secrets redacted line for line, to the family board's
`spills/`, kept until `yi sessions rm`, and ends `[full output: <path>]`; with nowhere to tee,
raw text returns. A contained call runs under
Seatbelt (§8).

- Owner: [`jobs.rs`](../crates/tools/src/jobs.rs), [`reduce.rs`](../crates/tools/src/reduce.rs).
- Settled by: D161, D221, D340.

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
The kernel shells out: `rlm.mcp.list_tools|call_tool|reload|close` run `$YI_BIN mcp … --json`;
`connect` alone goes through the host, from `~/.yi/mcp.json`. A stdio server starts in `~/.yi`,
outside every sandbox's writable roots, so its command is an absolute path or on `PATH`.

- Owner: [`mcp-cli`](../crates/mcp-cli/src/lib.rs). Settled by: D36, D71, D296.

### 7.7 Checkpoints
A shadow gitdir `~/.yi/checkpoints/<xxh32 of project>/`, the project as work tree, one lock per
gitdir. `capture` = `add --all` + `write-tree`. `restore` moves only the paths in the turn's
start-to-end tree diff (`--no-renames`); a path changed after the turn is kept and named; a start
with no paired end restores unscoped and says so. Turn start and end capture into
`custom{checkpoint}`; `/undo` and `yi undo` restore the last turn start. Without git, a no-op.
A shadow gitdir whose project has been gone 7 days is deleted, renamed aside first so a cut-short
delete never leaves a half repo; a project that cannot be stat'd, or whose nearest surviving ancestor is a mount
root (`/Volumes`, `/mnt`, `/media`, `/run/media`: an unmounted volume), is kept. A live project's repo is never deleted, only `git prune`d at 30 days once idle an hour,
which keeps every younger capture and the objects it names: captures write no ref, the shadow
index roots the newest, and each capture refreshes its tree's mtime because `write-tree` hands
back a cached tree untouched. The sweep is [`retention`](../crates/runtime/src/retention.rs):
about hourly behind `~/.yi/retention.stamp` from session start, and on demand as `yi gc` (a
preview) or `yi gc --fix`, one `kept of found, removed, trimmed` line per store.

- Owner: [`tools`](../crates/tools/src/checkpoint.rs), [`types`](../crates/types/src/checkpoint.rs).
  Settled by: D248.

### 7.8 Skills
Roots `{.yi,.agents,.pi,.claude}/skills` under cwd, then home; first root wins a name;
`<name>/SKILL.md` walked 2 levels. The catalog lists every repository skill and every
`~/.yi/skills` skill, but another home-root skill only when config `skills.global` names it; a
child's lists none of those. Frontmatter at discovery; the catalog clips each description at 60 chars and the
whole skill is `read("yi://skills/<name>")`; `$name` or a `trigger:` needle in a message the
user typed points at it as `Relevant: yi://skills/<name>`. Bundled: `skills/yi` (`just install-skills`), and Python skills `attach-image`,
`compact`, `goal`, `memory` shipped in the binary for the kernel venv.
A plugin's skills (`yi plugin add`, §17.1) are read the same way but never listed: one Yi skill
points at them (`skills/yi/ripwire`), and its `requires: ripwire` lists it only while that
plugin's skills are installed. A skill written for one repository says `within: <path>` and is
listed, triggered, classified and read only where the repository holds that path (`gate`,
`land`, `pr-review` name Yi's own scripts). `read("yi://skills/<name>/<file>")` serves a file beside a skill's
`SKILL.md`; a path that would leave the skill's directory is refused.

- Owner: [`skills.rs`](../crates/runtime/src/skills.rs). Settled by: D139, D290, D292.

## 8. Permission
A pure `decide` over the call, mode, rules, grants, holds and catastrophic context.

- Modes `Ask`, `Auto`, `Yolo`; default `permissions.mode`, else `Auto` (`--confirm`, `--auto`, `--yolo`); one prompt
  fragment each. Order: catastrophic (every mode), configured deny, session rule or grant,
  configured allow/ask, hold, mode. `Auto` allows reads, in-tree writes and provably safe
  commands, contains the unproven, asks for destructive and egress segments. Commands are
  decided per segment; an unparseable command is its own input.
- `git_dirs` reads `.git`, `gitdir:` and `commondir` with bounded reads, trusting a pointer only
  to a `HEAD`; catastrophic matching covers every spelling and reads through wrappers.
- A contained failure is remembered by the path the sandbox denied, and a later write under its
  directory asks (in home, any mention); with no path, `refused_scopes` remembers the program and
  verb and a later use asks. "Always allow" keeps a `Grant` (directory, tree root, program+verb),
  in memory, ≤ 1024 rules.
- Sandbox: Seatbelt `/usr/bin/sandbox-exec`, macOS only; writable cwd, git dirs minus
  `hooks config commondir gitdir`, session dir, tmp; reads deny the credential stores the read gate
  refuses (`~/.ssh ~/.gnupg ~/.aws ~/.kube ~/.docker ~/.yi/mcp/tokens ~/.yi/providers/tokens
  ~/.yi/oauth ~/.config/{gh,fgj,gcloud} ~/.netrc ~/.git-credentials ~/.npmrc ~/.pypirc
  ~/.cargo/credentials[.toml] ~/.password-store`, `credential_stores`, D323, D324) and the wall's `deny_read`, each
  bound by its resolved path, unwritable, and with its parents inside a writable root
  unrenamable; the kernel and its `bash()` take the wall too; a contained process starts through
  `env -u` for every inherited variable a name heuristic marks as a secret (D324); in auto mode the
  bash read belt judges a store argument by identity, a directory above one and a glob that reaches
  one; loopback the only network, and no unix socket (#599). Without it, `Contain`
  becomes a reviewable `Ask`.
- With a classifier and its `laya` key (minted by `yi serve`), `classifier.approval` says when the
  sidecar's `noul` answers a reviewable auto-mode ask: `instant` (default) first — P(safe) ≥ 0.9
  (0.98 if destructive) allows, ≤ 0.05 asks the user, else the reviewer; `after-delay` once the
  person has not answered for `askTimeoutSecs` (30; 0 never hands it over), a confident allow only, and only
  where a prompt closes when its call settles elsewhere (the solo TUI); `wait-for-user` never.
- With `models.autoReview` set, a reviewable ask goes to the reviewer (30 s); non-allow denies
  with a request id `ask_user` replays; `ActionLedger` (256) makes an approval single-use.
- Every settled ask is journaled as a `permission` custom entry: the ask, the verdict, and
  whether the user, the reviewer or nobody answered.
- Owner: [`decide`](../crates/permission/src/decide.rs), [`sandbox`](../crates/tools/src/sandbox.rs)
- State: `PermissionMode { Ask, Auto, Yolo }`, `Decision { Allow, Contain, Deny, Ask { title,
  description, reviewable } }`, `Class { Safe, Destructive, Egress, Unknown }`.
- Shapes: [`types`](../crates/types/src/permission.rs). Settled by: D15, D26, D81, D205, D206, D207, D293, D301.

## 9. Kernel
A persistent IPython process per session that reaches the host only through host requests.

- One `python -m ipykernel_launcher -f <connection file>` per session over Jupyter protocol 5.3
  on the pure-Rust `zeromq` crate; frames after `<IDS|MSG>` are HMAC-SHA256 signed.
- It boots on the first `ipython` cell, or in the background at session open under
  `kernel.prewarm` (default true); a failed prewarm is silent and the next cell reports it.
- It runs under the session Seatbelt profile plus `~/.yi/harness` and `~/.yi/mcp`; a cell's
  wall clock is `cell_ceiling`, default 600 s, started after boot.
- Admission (D285): a boot first takes one of the node card's `slots` (`~/.yi/node.json`, the
  cores less one clamped 1..=8 on first use, `node.slots` in config over it), a flock on
  `~/.yi/node/slots/<n>.held` that the kernel's process monitor holds until the process is
  reaped, so any exit, a crash or a killed host included, frees it. With every slot held the
  boot waits, cancellable, and the wait and the cell's result both say `node: N of N slots held
  (node.slots=N); this kernel waits for one` with each holder's pid and directory; a prewarm
  never waits. A kernel whose family already holds a slot shares it and never waits, so a parent
  cell awaiting its child cannot deadlock the node; the family fuses (§11) bound what shares it.
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
| `wiring.rs`, `memory/mod.rs`, `history.rs` | `fetch`, `history.grep`, `history.search`, `compact.run` (schedules only), `compact.status`; `memory.save/read/search/forget` |
| `subagent.rs` | `rlm.run` (returns at admission), `rlm.result/wait/status/list_subagents/delete_subagent/merge_worktree/discard_worktree/find_models`, `model.info`, parent-side `agent_message.send/request/list_agents` |
| `mailbox.rs`, `lease.rs`, `subagent/service.rs` | child-side `agent_message.send/request/list_agents`, `rlm.receive`; `rlm.interrupt`, `rlm.revoke`; `rlm.service` |
| `schedule/mod.rs`, `goal/mod.rs`, `plan/mod.rs`, `plan/request.rs` | `rlm_heartbeat.list/create/update/delete`; `goal.get/create/update`; `plan.get`, `plan.op` |
| `kernel_tools.rs` | `tool.list` (the session's table, `ipython` and a root's `ask_user` named as excluded), `tool.call {name, args, cell}` |

- `tool.call` runs the session's own adapter, found at call time, so rules, wall and broker judge
  each nested call on its own arguments as they judge a model's. Calls keep the loop's order
  through one read-write lock: reads share it, every other kind takes it alone, since the loop
  orders them only by awaiting. It answers only a call whose `cell` (the execute request a
  `pre_run_cell` hook put in a context variable, which tasks inherit) is the request the agent's
  `ipython` cell holds the kernel as, recorded when the kernel takes it: a task that outlived its
  cell, even while a later cell runs, a call after an interrupt, a call naming no cell, or a cell
  typed into the console, even one running while the agent's cell waits, is refused `cancelled`. A failure replies `{ok: false, kind, message, suggestions}`: `unknown_tool`,
  `excluded_tool`, `invalid_input` (also a cell's own `kernel://` read, which would queue behind
  the cell), `refused` (a denial), `tool_failure`, `cancelled`.
- Every call that ran is a `NestedCall` row on its cell's `ipython` result, `details.calls`: the
  files it named, a sequential call's `command`, an `ask_user` answer, never a write's content.
  A read row is kept once per tool and files, a cell keeps 256 of them, counting the rest in
  `callsDropped`, which the summary's `<read-files>` names in a `[…]` row (the paths are not kept;
  the compaction's details carry the count to the next one); every other row is kept. A
  call that finished after its cell closed is `outlived` on a later cell's result, even one already
  open. Compaction's file lists and the todo landed count read the rows, a cell that raised
  included; the landed count skips an outlived row, which an earlier turn started.
- A nested call runs as a program's (`ToolContext::program`, `AgentTool::execute_program`): the
  tool fills `ToolOutput::value`, a `yi_types::tool_value` shape the reply carries beside the
  text and the transcript never holds; a value's `cut` names everything left out (a fuse, a walk
  cap, a wall, binary skips). `grep` returns every hit from `offset` on, up to a 64 MiB fuse of
  file text and 100,000 matches, minting no tags and touching no paging state; `read` the asked
  window, an open one running to the end of the file, up to the same fuses in lines. A program
  read's lines are a program's to edit, never the model's (`Snapshot::program_lines`). A glob is
  its paths, no file opened, up to 100,000; a directory, `edit`,
  `write`, `bash` (streams before the reducer, the job, the polled output), `get_context` (layers
  unclamped), `todo` (the list), `annotate` (the record), an address (the page, and the 1-based
  `nextOffset` the tool takes back), a child's `ask_user`, an exec tool (its JSON) and `plan` (the
  plan document after the op, read back unjournaled) each have one. The line selector never runs.

### 9.3 Python surface
- The preamble binds `rlm` (the module), `mcp`, `fetch`, `bash`. `@_public` builds `rlm.__all__`:
  async `host_request, run, service, find_models, list_subagents, delete_subagent, send,
  request, receive, followup, status, list_agents, wait, plan_op, interrupt, revoke, result,
  merge_worktree, discard_worktree, fetch`; sync `put, get, ls, bash`.
- `yi.tools.<name>(**args)` (or `yi.tools.call(name, /, **args)`) calls `tool.call` and returns a `Result` (`text`, the model's view;
  `details`); it raises `UnknownTool`, `InvalidInput`, `Refused` (widened from plans) or
  `ToolFailure`. `yi.tools.search` pages with a `[…]` row, and names what `excluded()` leaves out.
  `Result.value` reads by attribute or key (`hits.value.matches[0].line`), holding no method a
  key could shadow; `Result.data` is the same JSON for `json.dumps`; both are `None` for a tool
  with none.
- A public coroutine a cell never awaits runs once in `post_run_cell` and prints its value; a
  mail call (`send`, `followup`, `interrupt`, `revoke`) is a task scheduled at the call.
- Blackboard `<sessions dir>/rlm-<pid>/family/`: `rlm.put(name, obj)` writes `<name>.dill` and
  `<name>.json` `{name, owner, at, bytes, type, serializer}`; `get` and `ls` read them.
- Kernel env: `YI_BIN`, `RLM_SESSION_DIR` (sessions dir for a root, the child's dir otherwise),
  `RLM_FAMILY_DIR`, `RLM_GLOBAL_HARNESS_STATE_DIR`, `RLM_DEPTH`, `RLM_MAX_DEPTH` (unread).
- Snapshot: `<kernel dir>/<store id>.kernel-state.{dill,json}` is an index over per-variable blobs in
  `<store id>.kernel-state.blobs/`; only changed variables are rewritten. Dilled 1.5 s after an `ok`
  cell, flushed on dispose (≤ 5 s), restored at boot; ≤ 256 MiB, 16 MiB per variable.
- Owner: [`kernel/src/client.rs`](../crates/kernel/src/client.rs),
  [`bootstrap.rs`](../crates/kernel/src/bootstrap.rs),
  [`uv_install.rs`](../crates/kernel/src/uv_install.rs),
  [`runtime/src/kernel.rs`](../crates/runtime/src/kernel.rs),
  [`rlm/__init__.py`](../python/yi_runtime/src/rlm/__init__.py)
- Shapes: [`types/src/kernel.rs`](../crates/types/src/kernel.rs)
- Settled by: D87, D164, D171, D176, D211, D232, D235, D238

## 10. Url and fetch
`Url` is the one reference type; `Resolver` turns one into text and logs every read.

- `Url { scheme, path, fragment }` serializes as `scheme://path[#L<start>-<end>[@<TAG>]]`.
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
| `history` | `<agent>[/<entry>\|/tail/N\|/since/<seq>][/custom/<type>]` | a transcript; `self` is the reader's; a root session's id (its file is `<created>_<id>.jsonl` exactly) resolves in any repository; only once no id along the path opens one, a name (its name fact, else its first prompt's title, case aside and spaces read as dashes) resolves in this repository first and then any other, and `<root>/<child>[/…]` a child per level by the name or id its parent's `custom{child}` trail records; a name several sessions carry is refused listing up to 8 `history://<id>` addresses with their repositories; the wall judges the session's id and name as well as the address as written, a history wall turns names and `root/child` off with a `Denied` that says so, and a reader's desk says it opens only itself and its parent |
| `checkpoint` / `mcp` | `<tree>/<path>` / `<server>/<uri>` | a checkpoint-tree file (§7.7) / an MCP resource (§7.6) |
| `family` / `tree` | `<name>` / `<agent>/<path>` | a blackboard sidecar / a member's checkout file, under `deny_read` |
| `yi` | `tools/<name>` / `skills/<name>` / `skills/<name>/<file>` | a deferred tool guide (§7.1) / a skill's `SKILL.md` / a file beside it (§7.8), under `deny_read` |

- Owner: [`runtime/src/fetch/mod.rs`](../crates/runtime/src/fetch/mod.rs),
  [`runtime/src/fetch/schemes.rs`](../crates/runtime/src/fetch/schemes.rs)
- Shapes: [`types/src/url.rs`](../crates/types/src/url.rs)
- Settled by: D98, D164, D213

## 11. Child
A detached `AgentSession` admitted by `SubagentHost` under a lease, a wall and a standing.

- Every child enters through `SubagentHost::admit`; `rlm.run` answers
  `{rlm_child_id, next, name, session_dir, model}` at admission and the run proceeds detached.
- kwargs are a whitelist: `name, model, thinking, fork, isolation, deny_write, deny_read,
  deny_url, context, check, deadline_s, tokens, parent_close, role, partition, tools, turns,
  schema`;
  `fork=all` refuses a model.
- A child's request is made of its brief (D307). `partition` is a list of Urls resolved at spawn
  through the parent's `Resolver` (walled as its reads, `kernel://` refused) and inlined before the
  prompt as numbered lines in untrusted yard fences, 64 KiB of lines at most, cut at a whole line
  with a `[… kept …]` row naming the cap and the rest. A worker spawn that names no `role` is
  `role="reader"` (D308); a plan delegation, a juror and a service are `root`. A reader is a
  question-child: `prompts/reader.md` is its whole system prompt, `tools` a subset of `read` and
  `grep` (both by default), `turns` its request cap (3, at most 10; the last is the last word, whose tool calls are refused,
  after a `[turns]` note; one turn is one request with no tools), `deny_write` gains `.`, the user's
  gate rules bind it, and it has no extension, kernel, plan, schedule,
  checkpoint or environment block; a fork or an isolation refuses; it counts toward the family cap
  like any child (D341). `rlm.ask(question, partition,
  schema=…)` runs one, reads its result and reaps it. A reader's `schema` is named in its
  question and sent as structured output only by a reader with `tools=[]` or `turns=1` (the turn cap's last word still carries the tools, so a reader that keeps its tools gets the schema in its question and `result`'s check only), strictly only where it closes every object (D309); its
  partition is its own first message, marked `shared_through` when a sibling sent it within 300 s.
- A worker (`role="worker"`, D334) is the same brief with writes: `prompts/worker.md`, the
  permission mode, the project's instruction files and the language packs are its whole system
  prompt, `tools` a subset of `read`, `grep`, `edit`, `write`, `bash` and `get_context` (the first
  four by default), `turns` 12 (at most 40). Its writes pass its wall, the broker and the user's
  gate rules as a root child's do, reminder rules reach it, and it compacts. It holds a worker
  slot, may take `isolation`, and refuses `fork` and `check`.
- `isolation` is `none`, `worktree` or `container:<image>` (D286), from `rlm.run` or a plan
  delegation's `spec.isolation`. A container child claims the same lane, branch and merge as a
  worktree child; `docker run -d --rm` starts one container of the image over it at spawn, the
  lane bind-mounted at its own path with its git dirs and `.git` pointer read-only, run as the
  lane's owner, and the wall's `container` sends every bash call through `docker exec`; a killed
  call's processes in the container are killed before its job settles. It is removed when the child's record
  drops (reap, repossession, a failed spawn) and by name at the next spawn on that lane. A node
  whose card lacks `container` refuses before a lane is claimed; a missing image is pulled once
  with a notice.
- Admission refuses at lever `family.cap` (16, an eval run's levers raise it to 128) live sessions,
  at depth `rlm.maxDepth` (1, clamped 1..=3) but for a juror, at `family.max_children` (8, up to
  128) workers, and on a taken name (D344).
- `Standing { Worker, Juror, Reader, Service }`: juror, reader and service stand outside the worker cap; lease,
  wall and family cap bind them all. A service (`rlm.service(name, brief, restart=3)`)
  respawns on its `ChildRecord` after `Failed { Provider | KernelDeath }`, ≤ 10 per 10 min.
  A judge seats `policy.n` jurors from the cheapest other model family, else `Abstain` (§13).
- The wall only reduces and `under(parent)` makes it hereditary; it refuses at the tool adapter
  before permission, `deny_read` implies write-deny, and bash naming a denied path is refused.
  A walled session also walls the old spill root and every session store and spares its
  family's spill dir and its own transcript, at the read gate and in its contained profile (D345).
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
  `LeaseRecord`, `ParentClose`), `ChildTrail` (the parent's `custom{child}` lines); child dirs
  `<parent file without .jsonl>/children/sub-<8 hex>` at the root, `<parent dir>/children/sub-<8 hex>`
  below it, `rlm-<pid>/sub-<8 hex>` for a root with no transcript (D317)
- Settled by: D165, D210, D215, D216, D218, D234, D307, D308, D309, D317

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
- A letter to another session: a root sends to `history://<id or name>`, resolved as a read of
  it is and judged by the same wall; the target must be a root, a transcript directly under a
  repository directory of the sessions store. It is appended to `channel://mail-<id>`, receipt
  `stored`. `from` is the sender's `history://<id>`, its reply address; `origin` points at the
  sender's entry, `intent` at the words behind its run. A `ref` in the reader's own store
  (`family://`, `kernel://`) is refused.
- Each durable root subscribes to its own mail channel at bind, from the channel's start, with
  no heartbeat source, so `/heartbeat clear` leaves it: one todo per letter, any number open,
  `FollowUp`; an `inform` waits for the next turn. A sender in the same process makes it due at
  once; another process's waits up to a minute. The wake shows the envelope as fenced data.
  A subscription to another session's mailbox is refused, in any letter case. A mailbox holding
  256 unread letters refuses the next one by name, so mail for a session nobody runs is bounded.
- Only a root sends outside; a child is refused and asks its root. `request` refuses another
  session, whose reply comes back as a letter. The envelope must fit the 16 KiB channel cap.
- `hops`: walking back to what started the run, a mail wake gives its highest letter's hop and
  intent, the user's words or the task give 0, and family mail and notices are passed over, so a
  child's report keeps the chain. Another wake, a clock's, gives 0 only when the letter before it
  is over an hour old, so a clock between a letter and its reply cannot reset a live chain. A
  letter carries that plus one; past `mail.hopLimit` (20) the send is refused with `yi.HopLimit`.
- State: `Kind { Inform, Request, Reply, Progress, Failure, Cancel }`,
  `Delivery { Queued, Woken, Inboxed, Answered, Stored }`.
- Owner: [`runtime/src/mail.rs`](../crates/runtime/src/mail.rs),
  [`mailbox.rs`](../crates/runtime/src/mailbox.rs), [`post.rs`](../crates/runtime/src/post.rs),
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
  Failed{cause, last}, Abandoned, Other }`, `BlockedOn { Child, User, External{probe}, Channel{address, filter?}, Other }`,
