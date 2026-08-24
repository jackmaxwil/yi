# yi mcp: MCP command-line client

`yi mcp` maps MCP operations to shell commands. Discover the right tool on
demand with `grep`, then generate shell commands (ideally with `--json`)
instead of carrying tool definitions in context.

## Mental model

1. **Connect once** to a server — this creates a named `@session` whose
   metadata and tool snapshot are saved. There is no resident process: every
   command connects, runs, and exits.
2. **Run commands against the `@session`**: list and call tools, list
   resources and prompts, ping.
3. **Default output is human-readable**; add `--json` for machine-readable,
   MCP-spec-shaped output that composes with `jq` and shell pipelines.

## First steps

```bash
yi mcp connect ./.mcp.json:fs @fs      # connect one config entry, name the session
yi mcp connect myserver                # entry "myserver" from ~/.yi/mcp.json, auto-named
yi mcp @fs tools-list                  # list tools
yi mcp @fs tools-call read_file path:="README.md"
yi mcp grep "search"                   # find tools across all sessions (no server contact)
```

## Connecting

Server formats accepted by `connect`:

- `<name>` — entry in `~/.yi/mcp.json` (or `.mcp.json`, `.vscode/mcp.json`, `.cursor/mcp.json`)
- `<config-file>:<entry>` — a single entry from a config file
- `<config-file>` — the file's only entry

Stdio (command-based) entries launch a local process for the duration of one
command — only connect to configs you trust. Session names auto-generate from
the entry name when `@session` is omitted.

## Session states

- **live** — last command succeeded; ready to use
- **connecting** — a connect is in progress
- **disconnected** — closed with `close`; `restart @session` to revive
- **unauthorized** — server rejected the connection (auth); fix credentials, then `restart`
- **expired** — server dropped the session; `restart @session`

Sessions are never auto-removed.

## Discovering and calling tools

```bash
yi mcp @fs tools-list                  # list tools
yi mcp @fs tools-get read_file         # one tool's schema
yi mcp grep "file" -m 5                # search cached tool names/descriptions + instructions
                                       # exits 0 on match, 1 on no matches (grep convention)
yi mcp @fs tools-call <tool> key:=value      # httpie-style args (auto-parsed JSON values)
yi mcp @fs tools-call <tool> '{"key": 1}'    # inline JSON object
echo '{"key": 1}' | yi mcp @fs tools-call <tool>   # stdin
```

To force a string value, quote it: `id:='"123"'`.

Schema snapshots let scripts fail early on breaking server changes:

```bash
yi mcp @fs tools-get read_file --json > expected.json
yi mcp @fs tools-get read_file --schema expected.json --schema-mode strict
```

`--schema-mode compatible` (default) allows the server to add fields; `strict`
requires byte-identical schemas.

## Auth (HTTP servers)

Remote servers may require OAuth. Login is a **user action** — a 401 never
opens a browser; the error names the command. Ask the user to run:

```bash
yi mcp login <server-url>        # browser flow; tokens go to the OS keychain
yi mcp logout <server-url>       # delete stored tokens
```

After login, `connect` and session commands attach credentials automatically
(`--profile <name>` selects among multiple logins; `--no-profile` skips).
Tokens refresh transparently; a session stuck in `unauthorized` needs a new
login.

## Output

`--json` prints one MCP-spec-shaped JSON document on stdout; errors go to
stderr. `--max-chars <n>` truncates human-readable output. Exit codes: 0
success, 1 grep-no-match, 2 usage error, 3 server/connect failure.
