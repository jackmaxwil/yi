# Yi

A coding agent in one Rust binary. The model works inside a persistent Python kernel, where
subagents are function calls and plans are programs.

![Three subagents asked from one kernel cell](docs/media/fanout.webp)

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/jackmaxwil/yi/main/scripts/install.sh | bash
yi --version
```

It checks the release's sha256 and the owner's key, then installs `yi` to `~/.local/bin` and its
runtime to `~/.yi`; Apple-silicon Macs and x86_64 Linux, and `| bash -s -- --help` prints that key. By hand:

```bash
v=X.Y.Z t=aarch64-apple-darwin; base=https://github.com/jackmaxwil/yi/releases/download/v$v; a=yi-$v-$t.tar.gz
curl -fLO "$base/$a" -O "$base/$a.sig" -O "$base/SHA256SUMS"; shasum -a 256 -c SHA256SUMS --ignore-missing
echo 'jackmaxwil-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAaFfxFxxkIpL0TuZcvkcLpzT164hwIaCbzyFnVkAMX7' > allowed_signers
ssh-keygen -Y verify -f allowed_signers -I jackmaxwil-release -n yi-release -s "$a.sig" < "$a"
tar -xzf "$a" && "./yi-$v-$t/install.sh"
```

## Quick start

```bash
yi ask --model faux/faux-1 "hello"   # offline, no key: prints "faux: hello"
yi login openrouter                  # paste a key at the prompt
yi setup                             # default model and permission mode
yi "what does this repo do?"         # one answer
yi                                   # the console; --solo: one chat
```

## Log in

```bash
yi login openrouter      # also openai, google: an API key, pasted or piped on stdin
yi login cursor          # browser sign-in; --no-browser skips opening it
yi login kimi            # also anthropic, openai-codex: a profile you write first
```

The OAuth three read `~/.yi/oauth/<provider>.json` ([templates](docs/examples)); tokens land in
`~/.yi/providers/tokens/<provider>.json` (0600), not the config, and `*_API_KEY` wins when set.

## Plugins and skills

```bash
yi plugin list
yi plugin add --all                  # or one: ripwire, chrome
yi plugin sync
```

`ripwire` 0.6.5 is a code map (an edit's broken callers, the code a task touches; 16 skills);
`chrome` 155.0.8059.39 is a headless shell for the browser pane, both pinned in `~/.yi/plugins`.

A skill is a directory with a `SKILL.md`, read from `.yi/skills`, `.agents/skills`, `.pi/skills` or
`.claude/skills`, then `~/.yi/skills`; `$name` points the model at one.

## Configure

```bash
yi debug config                       # ~/.yi/config.json, secrets redacted
```

Strict: an unknown key is an error that names it. Every key is in
[YI_DESIGN.md §17.1](docs/YI_DESIGN.md).

| key | default | meaning |
|---|---|---|
| `model`, `thinking` | none, `medium` | `provider/id`; `off` to `max` effort |
| `models.summarizer`, `.advisor`, `.autoReview` | main, off, off | compaction, titles; naming one turns it on |
| `models.classifier` | off | TypeSafe's Jev through OpenRouter (`typesafe/jev-1.13`) as the permission and skill classifier; typed messages and judged commands leave the machine |
| `routing` | none | OpenRouter's `provider` object |
| `permissions.mode`, `permissions.allow` | `auto`, none | `ask`, `auto` or `yolo`; calls that run unasked |
| `tui.pace` | 100 | reveal speed, percent |
| `rlm.maxDepth` | 1 | subagent nesting, 1 to 3 |
| `kernel.prewarm`, `mcp.enabled`, `lanes.enabled` | true, false, true | kernel at open; the MCP CLI; a worktree per root |

| environment | for |
|---|---|
| `YI_PREFIX`, `YI_VERSION`, `YI_RELEASE_BASE` | install directory; the release it reads |
| `YI_KERNEL_VENV`, `YI_KERNEL_PYTHON`, `YI_BIN` | the kernel's venv; the `yi` it calls |
| `YI_TRACE`, `YI_CLAUDE_HOOKS`, `YI_CLAUDE_HOOKS_FILES` | a profiler trace; Claude Code tool hooks |
| `YI_LEVERS`, `YI_FIXED_IDS`, `YI_BLESS`, `YI_FAKE_RIPWIRE`, `YI_WARM`, `YI_REVIEW_MODEL`, `YI_ROUND_AUTHORS`, `YI_SIGNING_KEY`, `YI_ALLOWED_SIGNERS` | development: levers, ids, goldens, fakes, review bot, signing |

| verb | |
|---|---|
| `ask` `rpc` `acp` `serve` `console` `tui` | one turn; stdio protocols; daemon; workspace; solo TUI |
| `plan` `todo` `why` | plan ops; the newest todo list; a line to its goal |
| `sessions` `stats` `memory` `fetch` `catalog` | sessions; token stats; memories; one URL |
| `undo` `lanes` `trust` `gate` `doctor` `gc` `debug` | undo; worktrees; trust; policy; repair |
| `login` `logout` `setup` `plugin` `update` `version` `mcp` | credentials; plugins; release; MCP |

Flags: `--model`, `--json`, `--schema`, `--yolo`, `--continue`, `--deadline`;
`yi --help` has all.

## Plans

```console
$ yi todo                             # the newest todo list
$ yi why crates/cli/src/main.rs:42    # walk a line back to the words that asked for it
```

A plan is a goal and a graph of todos, from the `plan` tool or a cell's `Plan.create`; a todo with a
contract closes only when its check passes. Design: [YI_DESIGN.md §13](docs/YI_DESIGN.md).

## Subagents

```python
answers = await asyncio.gather(*(rlm.ask("Can this function panic? Quote the line.", [url]) for url in urls))
h = await rlm.run("Port the parser to the new token type", role="worker", isolation="worktree")
result = await h.result(timeout=420)
```

A `reader` gets `read`, `grep` and three turns; a `worker` edits too; a `root` is a session.
`isolation="worktree"` gives a child a worktree. Design: [YI_DESIGN.md §11](docs/YI_DESIGN.md).

## Use Yi from programs

```console
$ yi ask --model faux/faux-1 --json --no-deltas hello | jq -r .type | tr '\n' ' '
agent_start turn_start message_start message_end message_start message_end turn_end agent_end final
```

`yi ask` exits 0 with an answer, 1 when the run failed, 2 when refused before starting, 3 when the
answer misses `--schema`, 4 on a denied tool or a quota:

```bash
if yi ask --model faux/faux-1 --schema '{"type":"object"}' "list the risks as JSON" > risks.json
then echo "risks.json written"; else echo "yi exited $?"; fi   # yi exited 3: faux answers no JSON
```

`git diff | yi ask -` reads the prompt from stdin; `--continue` resumes the newest session here.
With `{"mcp": {"enabled": true}}`, `yi mcp --json` reaches an MCP server once.

## SDK

```bash
yi ask --model faux/faux-1 --json "hello"   # the event stream
yi acp                                       # ACP v2
yi serve                                     # the daemon's socket
```

A full SDK is coming; today build on those, or on `~/.yi/sessions/` (Pi's v4 JSONL).

## Contributing

```bash
just check                            # fmt, clippy, the guardrails, the tests
```

A private forge is the source of truth; this is a read-only mirror of its `main`, and contributions
come by invitation ([CONTRIBUTING.md](CONTRIBUTING.md)).

## Built on

Session, tool, kernel, URL, child, todo, plan, contract and lane compose every feature
([YI_DESIGN.md §3](docs/YI_DESIGN.md)), and every cut of model-facing text names its cap.

`auto` mode sandboxes commands with Seatbelt; `.git` and credential stores stay denied even in
`yolo`.

## Develop and license

```bash
just dev                              # build and start yi
just test-quick                       # the workspace without its slowest tests
just build-dist                       # the release build
```

[YI_DESIGN.md](docs/YI_DESIGN.md) is the law, [ARCHITECTURE.md](docs/ARCHITECTURE.md) the map.
Apache License 2.0 in `LICENSE`; copyright Jack Mazac and AiBridge LLC, in `NOTICE`. `vendor/` keeps its upstream licenses.
