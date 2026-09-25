# Architecture in one page

Yi is a coding agent in Rust: one `yi` binary, a persistent Python kernel beside it, and sessions
stored in Pi's v4 session JSONL format. [YI_DESIGN.md](../YI_DESIGN.md) is the law; this page is
a map into it and states nothing the design doc does not.

## Where to read

| Question | Section |
|---|---|
| What is in and out of scope | [§1](../YI_DESIGN.md#1-scope) |
| Which crate owns what, and which edges are allowed | [§2](../YI_DESIGN.md#2-crates-and-dependency-direction) |
| The primitives and the capabilities composed from them | [§3](../YI_DESIGN.md#3-primitives-and-composition) |
| Session file, loop, run queue, compaction, interrupt | [§4](../YI_DESIGN.md#4-session) |
| Providers and the model catalog | [§5](../YI_DESIGN.md#5-provider) |
| System prompt and extensions | [§6](../YI_DESIGN.md#6-prompt) |
| Tools, editing, bash jobs, checkpoints, skills | [§7](../YI_DESIGN.md#7-tool) |
| Permission modes, rules and the sandbox | [§8](../YI_DESIGN.md#8-permission) |
| The kernel and its host requests | [§9](../YI_DESIGN.md#9-kernel) |
| References and `fetch` | [§10](../YI_DESIGN.md#10-url-and-fetch) |
| Children, mailbox, plans and contracts, lanes | [§11](../YI_DESIGN.md#11-child)-[§14](../YI_DESIGN.md#14-lane) |
| Goal, schedule, advisor | [§15](../YI_DESIGN.md#15-goal-and-schedule), [§16](../YI_DESIGN.md#16-advisor) |
| CLI, ACP daemon, TUI, console | [§17](../YI_DESIGN.md#17-surfaces) |
| Dependencies, size, code style, schemas, gates | [§18](../YI_DESIGN.md#18-dependencies-and-size)-[§21](../YI_DESIGN.md#21-guardrails) |
| Memory | [docs/memory.md](../memory.md) |
| Benchmarks and eval gates | [evals/README.md](../../evals/README.md) |
| Why a decision holds | the decision log in [ARCHITECTURE.md](../ARCHITECTURE.md), one ADR each in [adr/](adr/) |

## One request, end to end

A surface (§17) hands a prompt to `AgentSession`, which returns at admission and queues it (§4.3).
The loop (§4.2) compacts when due and assembles context (§4.4), streams one assistant message
from the provider (§5), and passes every tool call through the permission decision (§8), in a
child after its wall (§11), before the tool runs (§7). Each message is appended to the session file
(§4.1) and broadcast as an `AgentEvent` to every attached surface.
