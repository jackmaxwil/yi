# Yi

Personal coding agent. Native Rust. One binary, ~5.7 MB, starts in ~2.4 ms,
20 direct dependencies. Built to stay small enough for one person to
understand end to end.

## Run

```bash
yi                                     # the workspace: rail of sessions, chat panes
```

```bash
yi --solo                              # one chat, inline in the terminal
```

`yi` starts a daemon (`yi serve`) on `~/.yi/daemon.sock` if none is listening and
opens the workspace over it. The daemon owns every session and its worker; the
console renders. Sessions keep running while no console is attached.

Two ways out:

| keys | what happens |
|---|---|
| `⌥q` (or `ctrl+b q`, `/quit`) | the console closes; the daemon and its sessions keep running |
| `ctrl+c` `ctrl+c` | the first press warns, the second stops the daemon and every worker, then quits |

A console attached to a daemon from an older build says so in the pane: restart it
with `ctrl+c` `ctrl+c` and run `yi` again.

```bash
yi ask --model openrouter/z-ai/glm-5.3-flash "prompt"
```

```bash
yi ask --model faux/faux-1 --json "prompt"   # offline, scripted provider, no key
```

## The workspace

The rail on the left lists sessions newest first: a slot number, the session's
avatar, its state, and in the full sidebar (`⌥b` cycles rail → full → hidden) its
name and age. Over kitty or Ghostty the avatar is an identicon drawn from the
session id; everywhere else it is the same two initials on the same colour. The
session in front wears a tinted row.

| keys | |
|---|---|
| `⌥1..9` / `⌘1..9` | resume that rail slot into the focused pane |
| `⌥n` | new session in the focused pane |
| `⌥v` `⌥s` `⌥x` `⌥z` | split right, split down, close, zoom |
| `⌥←→↑↓` | move focus between panes (solo child focus when there is one pane) |
| `⌥t` `⌥]` `⌥[` `ctrl+b 1..9` | tabs |
| `⌥/` | palette: `e path` opens an editor pane, `md path`, `diff path`, `nb` |
| `⌥⇧j` `⌥g` | notebook pane, diff pane for the focused session |
| `Tab` | sidebar ↔ panes |
| `ctrl+b` | prefix for terminals that eat alt |

A chat pane is solo's chat, the same code: composer with history and paste
markers, `/` verbs, `@` files, `esc esc` for the entry tree, `/plantree`, the
permission popup (`y` / `a` / `n`), the working line and orb, the status row.

## Develop

The loop while working on yi itself, one line:

```bash
just dev
```

which is `cargo build -p yi-cli`, stop any daemon the previous build left
running, then `./target/debug/yi`. The stop matters: `yi` reuses a listening
daemon, and a daemon from an older build cannot stream to a newer console.

```bash
cargo build --profile dist -p yi-cli   # shipping binary: target/dist/yi
```

```bash
just check                             # fmt, clippy -D warnings, all guardrails
```

Proofs of the rendered UI live in `scripts/proof/`: `just tui-proof`,
`just console-proof scripts/proof/workspace.drive`, `just console-pty` (real
console under an xterm-kitty pty, counts the avatar and orb placements).

## Philosophy

An agent is a long-running program holding a moving picture of a codebase
inside a fixed budget. What makes it good happens where nobody demos: what
gets dropped when the window fills, what happens when a stream dies at 90%,
whether a denied command comes back with evidence, whether the transcript you
scroll is the transcript the model saw.

So the rules are structural, not aspirational. Every budget is enforced by
CI, not by intention. Ratchets only shrink: panics (zero), glob re-exports
(zero), duplicated prose (zero), binary size, startup time, dependency count,
test lines, comment volume, token spend. Growth is a deliberate commit of its
own, never a side effect. A schema lock makes every wire-shape change a
reviewed diff.

## Context efficiency

Context is a budget, not a buffer. Every source competing for the window —
project instructions, skills, tool output, prior turns — has a byte budget
and a truncation marker. Nothing gets to grow silently.

Compaction is prefix-aligned: the stable head of the conversation stays
byte-identical across turns, so the provider's prompt cache keeps hitting
instead of re-reading the whole session every turn.

## Token efficiency

The fixed prefix every request pays — system block plus tool table — is
measured by a test and ratcheted in CI. Adding a tool or a system sentence
fails the build until the growth is committed on purpose.

## Speed

- `yi --version`: ~2.4 ms measured, ≤ 5 ms budgeted. `--version`, `--help`,
  and `sessions list` return before config parse or runtime construction.
- The async runtime is a current-thread tokio, built per command. No thread
  pool warms up to print a version string.
- Dist binary ~5.7 MB, budget 6 MiB. Every dependency added logs its measured
  size and startup delta in `docs/size-ledger.md` before it lands.

## Memory

Nothing heavy exists until used. The Jupyter kernel compiles into every
build but boots lazily on the first `ipython` call — no Python process
otherwise. MCP is compiled in but runtime-gated off by default. Solo paints an
inline viewport on the normal screen: finished output is written once to
native scrollback and never repainted; a workspace pane paints the same chat
into its rectangle from the retained transcript.

Sessions live on disk as an append-only entry tree, not in RAM: branch,
rewind to any entry, resume after a crash, read with tools that are not this
program. Unknown fields survive round-trips, so a newer session still loads
in an older binary.

## Planning

Planned work lives on the forge, not in this tree: an issue is the identity of
a piece of work, and its number is what everything else cites. A feature pull
request names its issue and the merge closes it — nothing is marked done by
hand. Milestone dates are not typed; they are divided out of measured
throughput and rewritten every week.

## Subagents

Subagents are function calls, not protocol. Each session can own a
persistent Python kernel; from inside it, `rlm.run("prompt")` asks the host
to spawn a child agent. State survives between calls, snapshots to disk,
revives across restarts. Recursion is a language feature.

Child authority only shrinks: a spawn spec can fork none, all, or the last N
entries of the parent context, and overlays customize or reduce the child's
tools and model — never exceed the parent. Depth is capped. The kernel never
holds MCP sockets or tokens; kernel Python shells out to the one-shot
`yi mcp --json` CLI.

## Architecture

Fifteen crates, all in the default build, strict dependency order:

| crate | owns |
|---|---|
| `yi-types` | every serialized shape; the schema wall (serde only, no runtime) |
| `yi-loop` | the turn loop and interrupts |
| `yi-ai` | providers and the model catalog |
| `yi-session` | the entry tree, JSONL codec, tree operations |
| `yi-context` | projection, accounting, compaction, assembly |
| `yi-permission` | modes, rules, holds, the absolute denylist |
| `yi-tools` | the tool contract; files, search, shell, checkpoints, skills |
| `yi-kernel` | the Jupyter client: ZeroMQ, HMAC, the host bridge |
| `yi-runtime` | the session actor; subagents, schedules, goals, advisor |
| `yi-acp` | the editor protocol server, the `yi serve` daemon, the lossless `_yi/*` stream |
| `yi-orb` | the kitty orb: frames, RGBA, the graphics escapes |
| `yi-tui` | solo's chat: the app, its reducer, the port a host feeds it through |
| `yi-console` | the workspace shell: rail, panes, tabs, editor and notebook panes |
| `yi-mcp-cli` | one-shot MCP client, config-gated |
| `yi-cli` | the composition root |

The turn loop is under 1,000 lines and its public API returns no `Result` —
failure is a value in the event stream, not an exception climbing the stack.
Every surface (CLI, solo, workspace, editor protocol, daemon, RPC) is a client
of that loop rendering the same event stream; none is privileged. The daemon
path is lossless: the worker emits every runtime event verbatim as `_yi/event`,
so a workspace pane runs solo's reducer, not a second one.

Dependency direction is an allowlist checked in CI; an undeclared edge fails
the build. `unsafe_code` is forbidden in every crate. Newtypes cross every
crate boundary — no bare `String` or `u64` where an id or a unit exists.

Editing is by content, not coordinates: content-addressed lines make a stale
edit fail loudly instead of landing in the wrong place. Every touched file is
checkpointed into a shadow git directory; `undo` puts it back. Permission is
a decision with evidence: modes and rules decide, holds turn a match into a
question with a reason, a denial carries what it saw, and one tier — home
directory, device nodes, the workspace's own `.git` — no mode can override.

## Use

`yi acp` speaks the editor protocol. `yi serve` runs the daemon. `yi rpc`
streams framed JSON commands and events. `yi sessions list`, `yi undo`,
`yi mcp` (when enabled) round out the surface. `--json` on `ask` emits the
raw event stream for scripting.

## Configure

One file: `~/.yi/config.json`. Current keys:

```jsonc
{
  "model": "openrouter/z-ai/glm-5.3-flash",   // default model
  "models": {                                  // per-role overrides
    "primary": "...",
    "summarizer": "...",                       // compaction, cheaper is fine
    "advisor": "..."                           // naming this turns the reviewer on
  },
  "mcp": { "enabled": false },                 // MCP stays off until asked
  "bash": { "autoBackgroundMs": 0 },           // long commands auto-background
  "routing": { "preferred_min_throughput": { "p50": 20 } }, // OpenRouter provider object, verbatim; {} = none
  "gates": { "artifact": true, "closure": true },  // the two soft stop gates (D162); --no-gates on yi ask
  "console": { "autoSide": true },             // first kernel cell / tracked edit opens a side pane
  "tui": { "pace": 100 },                      // streamed-text reveal speed, percent; 0 paints on arrival
  "keys": { "ctrl+g": "some-action" }          // solo keymap overrides
}
```

Unset roles fall back to the primary model. `YI_*` environment variables are
a registered surface with a hard cap of 40.

## Contribute

Read `docs/YI_DESIGN.md` (the law) and `docs/ARCHITECTURE.md` (the map: version,
feature ledger, decision log; version history in `docs/CHANGELOG.md`) first.
Code follows the docs; revising a settled decision requires a decision-log row
in the same change.

- `just check` green before any claim of done — every gate judged by exit
  code, never by piped output.
- Baseline edits (`--update`) land in their own commit, never with code.
- Every test defends one externally observable contract; fixtures come from
  reference implementations, never from Yi's own output. A regression test
  is watched failing against the unfixed code before the fix is claimed.
- No `unwrap`/`expect`/`panic` outside tests. A comment earns its line by
  naming what the code cannot; two lines, hard cap.
- Agent instructions live in `.ruler/`; regenerate the per-tool files with
  `npx @intellectronica/ruler apply`. Never edit the generated ones.
