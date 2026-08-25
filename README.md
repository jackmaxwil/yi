# Yi

A coding agent I built for myself, in Rust, because I wanted one that stays
small enough to understand and still does the work.

The premise is that a coding agent is not a chat window with a shell attached.
It is a long-running program that has to hold a moving picture of a codebase in
a fixed budget, act on it, and be honest about what it did. Most of what makes
one good or bad happens in the parts nobody demos: what gets dropped when the
window fills, what happens when a stream dies at 90%, whether a denied command
comes back with evidence or just a refusal, whether the transcript you can
scroll is the same transcript the model saw.

So Yi is built around a few positions:

**A small core that stays small.** The turn loop is under a thousand lines and
its public API returns no `Result` — failure is a value in the event stream,
not an exception climbing the stack. Everything above it is a client of that
loop: the CLI, the editor protocol, the daemon, the terminal UI. Ceilings on
size are enforced by the build, not by intention.

**Context is a budget, not a buffer.** Every source that competes for the
window — project instructions, skills, tool output, prior turns — has a byte
budget and a truncation marker. Compaction is prefix-aligned so the stable head
of the conversation stays byte-identical across turns and the provider's cache
keeps hitting. Token spend is ratcheted in CI like any other resource.

**Durable, addressable sessions.** The transcript is an append-only entry tree
on disk. You can branch it, rewind to any entry, resume after a crash, and read
it with tools that are not this program. Unknown fields survive round-trips, so
a session written by a newer version still loads in an older one.

**Editing by content, not coordinates.** Files are read and written through
content-addressed lines, so an edit that no longer matches fails loudly instead
of landing in the wrong place. Every file the agent touches is checkpointed
into a shadow git directory, and `undo` puts it back.

**A real Python runtime, not a sandbox toy.** Each session can own a persistent
Jupyter kernel. State survives between calls, snapshots to disk, and revives
across restarts. Subagents are spawned from inside that kernel as ordinary
function calls, which makes recursion a language feature instead of a protocol.

**Permission as a decision, with evidence.** Tools declare their kind; modes
and rules decide; holds turn a matching call into a question with a reason
attached. A denial carries what it saw. One tier is absolute — the home
directory, device nodes, the workspace's own `.git` — and no mode, including
the reckless one, can talk its way past it.

**An advisor that reviews the work log.** A second, cheaper judgment pass reads
what the turn actually emitted — your instructions verbatim, the agent's own
prose, its stated intents — and flags unbacked claims and drift. Deterministic
signals run always and cost nothing; the model reviewer is opt-in and triggered.

**A terminal UI that respects the terminal.** An inline viewport on the normal
screen, native scrollback, no alternate screen. Finished output is written once
and never repainted, because you cannot observe where the user scrolled.

## Layout

Thirteen crates, all in the default build, in strict dependency order:

| crate | owns |
|---|---|
| `yi-types` | every serialized shape; the schema wall (serde only, no runtime) |
| `yi-loop` | the turn loop and interrupts |
| `yi-ai` | providers and the model catalog |
| `yi-session` | the entry tree, its JSONL codec, tree operations |
| `yi-context` | projection, accounting, compaction, assembly |
| `yi-permission` | modes, rules, holds, the absolute denylist |
| `yi-tools` | the tool contract; files, search, shell, checkpoints, skills |
| `yi-kernel` | the Jupyter client: ZeroMQ, HMAC, the host bridge |
| `yi-runtime` | the session actor, plus subagents, schedules, goals, advisor |
| `yi-acp` | the editor protocol server |
| `yi-tui` | the terminal UI |
| `yi-mcp-cli` | one-shot MCP client, gated off by config |
| `yi-cli` | the composition root |

Dependency direction is an allowlist checked in CI: an undeclared edge fails
the build. Nothing below the composition root reaches sideways.

Around them: a Python package the kernel loads, bundled skills, a vendored
output reducer for noisy commands, and the guardrail scripts that hold every
budget.

## Build

```bash
cargo build --profile dist -p yi-cli   # the shipping binary (target/dist/yi)
```

```bash
just check                             # fmt, clippy -D warnings, guardrails
```

## Use

```bash
yi                                     # the TUI, on a TTY
```

```bash
yi ask --model anthropic/claude-opus-4-5 "prompt"
```

```bash
yi ask --model faux/faux-1 --json "prompt"   # offline, scripted, no key
```

`yi acp` speaks the editor protocol, `yi serve` runs the daemon, `yi rpc`
streams framed JSON commands and events. Every surface renders the same event
stream; none of them is privileged.

## Guardrails

Budgets start at zero and only shrink: panics, glob re-exports, duplicated
production text, environment variables, dependency count, binary size, startup
time, test lines, token spend. Growth is deliberate and lands in its own
commit, never alongside the code that caused it. A schema lock file makes every
wire-shape change a reviewed diff.

## Docs

- `docs/YI_DESIGN.md` — the design: primitive tables, per-module contracts
- `docs/ARCHITECTURE.md` — version, changelog, feature ledger, decision log
- `docs/TODOS.md` — the open queue

Agent instructions live in `.ruler/`; `npx @intellectronica/ruler apply`
regenerates the per-tool files. Study checkouts under `ref/` are gitignored.
