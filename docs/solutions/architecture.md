# Architecture in one page

Yi is a personal native-Rust coding agent: Pi's core shape and wire formats,
prime-agent's runtime (Jupyter kernel, context management, subagents,
heartbeats), OMP's hashline editing, and a redesigned advisor.

## Crates

Thirteen crates under crates/ (twelve default + yi-mcp-cli behind cargo feature
mcp). Folder x/ is crate yi-x. Dependency direction is an allowlist in
scripts/guardrails/boundaries.toml.

| crate | owns |
|---|---|
| yi-types | every serde shape: messages, entries, events, model/tool wire. The schema authority; deps = serde only |
| yi-loop | pure run_loop + interrupt module; no Result in its public API; <= 1,000 lines |
| yi-ai | provider adapters (anthropic-messages, openai-completions incl. OpenRouter, faux), SSE decoder, JSON salvage, message transform, retry policy, bundled model catalog |
| yi-session | Pi v4 entry-tree store: mutation-log replay, JSONL + memory repos, torn-tail repair, fork/branch, conformance-tested against Pi fixtures |
| yi-context | context primitives P2-P18: projection, chars/4 accounting, compaction policy/cut/prompts, retention floor, window chain, ledger reader, source budgets, world-state diffs, convertToLlm |
| yi-permission | pure decide() with fixed precedence, catastrophic denylist (all modes incl. yolo), sha256-sealed session rules, holds, mode prompt fragments |
| yi-tools | Tool trait + builtins: bash, glob, grep, write, hashline read/edit (line+tag addressing, brace block resolver, snapshots, prepare/commit patcher) |
| yi-kernel | Jupyter/ZMQ client (phase 4) |
| yi-runtime | AgentSession composition; subagent/schedule/advisor as modules; the only LoopConfig constructor; the glue that keeps yi-loop and yi-ai independent |
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
- Wire schemas evolve additively only; fixtures never get deleted (design 19).
