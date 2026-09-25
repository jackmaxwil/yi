# Architecture in one page

Yi is a personal native-Rust coding agent: Pi's core shape and wire formats, a
Jupyter-kernel runtime (context management, subagents, heartbeats), hashline
editing, and a redesigned advisor.

## Crates

Thirteen crates under crates/, all in the default build; yi-mcp-cli is
runtime-gated by `mcp.enabled` config (D36), the kernel is compiled in and
boots lazily (D38). Folder x/ is crate yi-x. Dependency direction is an
allowlist in scripts/guardrails/boundaries.toml.

| crate | owns |
|---|---|
| yi-types | every serde shape: messages, entries, events, model/tool wire. The schema authority; deps = serde only |
| yi-loop | pure run_loop + interrupt module; no Result in its public API; <= 1,000 lines |
| yi-ai | provider adapters (anthropic-messages, openai-completions incl. OpenRouter, openai-responses, faux), SSE decoder, JSON salvage, message transform, retry policy, bundled model catalog |
| yi-session | Pi v4 entry-tree store: mutation-log replay, JSONL + memory repos, torn-tail repair, fork/branch, conformance-tested against Pi fixtures |
| yi-context | context primitives P2-P18: projection, chars/4 accounting, compaction policy/cut/prompts, retention floor, window chain, ledger reader, source budgets, world-state diffs, convertToLlm |
| yi-permission | pure decide() with fixed precedence, catastrophic denylist (all modes incl. yolo), sha256-sealed session rules, holds, mode prompt fragments |
| yi-tools | Tool trait + builtins: bash, glob, grep, write, hashline read/edit (line+tag addressing, brace block resolver, snapshots, prepare/commit patcher), ipython over a KernelBridge seam |
| yi-mcp-cli | one-shot `yi mcp` CLI: stdio + streamable-HTTP (ureq) transports, OAuth login/logout, session/snapshot stores, grep discovery |
| yi-kernel | Jupyter client over pure-Rust zeromq: HMAC framing, uv venv bootstrap, execute queue + iopub reducer, host.request comm bridge, interrupt/lifecycle, boot gate |
| yi-runtime | AgentSession composition; subagent (rlm.run depth 1, admission/attribution/notices, fork seeding, worktree isolation) with mailbox as its B6/B13 messaging half, goal + plan facts with host-verified completion, triggered rules, the wall (per-child capability reduction), KernelService provisioner + HostRegistry, schedule/advisor as modules; the only LoopConfig constructor |
| yi-acp | ACP v2 server (phase 5b) |
| yi-tui | inline-viewport TUI, feature-gated (phase 7) |
| yi-cli | the yi binary: ask and rpc today; acp/serve/sessions later |

## Data flow

yi ask / yi rpc -> AgentSession.prompt (returns at admission, run spawned) ->
run_loop: at each message boundary the compaction hook may replace the
in-flight history (summary + retained tail, appended to the store as a v4
Compaction entry) -> convertToLlm (L4: summaries/bash/custom become user
messages, internal-context wrappers recognized) -> ProviderStream (StreamFn
dispatching on model.api) -> adapter builds request, pumps SSE, maps to
AssistantMessageEvents -> loop assembles turns; every tool call passes the
PermissionBroker gate (catastrophic denylist > configured deny > session
rule > configured allow/ask > hold > mode) before executing; steer and
follow-up queues drain between turns -> MessageEnd persists to the session
store -> AgentEvent broadcast -> renderer (text deltas or JSON lines).

## Invariants (the absences that matter)

- The loop never learns about protocols, providers, or tool names.
- yi-types holds no channels, handles, runtime types, or a workspace error enum.
- yi-tui / yi-acp / yi-cli never depend on yi-tools, yi-ai, or yi-permission
  directly; nothing below yi-cli depends on yi-mcp-cli.
- Nothing writes the session store but the runtime (from phase 2 on).
- The kernel process holds no MCP sockets or SDK: kernel Python shells out
  to the one-shot `yi mcp --json` CLI (design §7.6).
- Wire schemas evolve additively only; fixtures never get deleted (design §20).
- A terminal claim is measured, never accepted: goal and task completion run
  their own check host-side (D52), and a child's structured result is validated
  at the seam that hands it back.
- A subagent overlay only ever *reduces* the child (deny lists, wall paths);
  parent authority is never replaced, and nothing a child says arrives as
  user-role text (D58).
