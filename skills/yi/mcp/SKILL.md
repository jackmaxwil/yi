---
name: mcp
description: The /mcp page, and yi mcp servers, to see and manage MCP servers
---

# MCP page

`/mcp` in the solo TUI opens the page in the bottom panel. In the console, `mcp` in the palette opens it as a pane. Both draw the same page: a status header (`mcp.enabled` on or off, server count), one row per server, a detail block for the selected row, and a key footer.

Reach for it to see which servers are configured, where each came from, whether a login is due, and how many tools a probed server offers. To drive a server, use `yi mcp` in bash (see `yi mcp help`).

## Rows

Servers come from `~/.yi/mcp.json`, `.mcp.json`, `.vscode/mcp.json` and `.cursor/mcp.json`, in that order. The glyph is the session state: `●` live, `!` login needed or expired, `◌` no session yet, `✕` disabled in its file. The tools column is `—` until a probe has connected the server.

The same rows are one call away: `yi mcp servers --json` prints a list of `{name, source, path, transport, target, enabled, session, state, tools}`. A file that cannot be read is one row with `error` and no name.

## Keys

| Key | Does |
|---|---|
| ↑ ↓ | select a server |
| e | flip `mcp.enabled` (takes effect next session) |
| p | probe: connect the server, then list again so the tool count fills in |
| l | OAuth login, HTTP servers only; the browser opens and the page waits |
| o | open the selected row's config file with the desktop opener |
| r | reload the list |
| esc | close |

While a probe or login runs the header says so and further letters wait. A failure shows on one red line under the header, with the server's own stderr.

## When it shows nothing

- `mcp.enabled off`: the list stays empty until `e` turns it on.
- `no servers in …`: none of the four files declares one. Entries live under `mcpServers` or `servers`.
- Auth reads `unknown` for an HTTP server never connected: press `p`, and `l` if it then says `login needed`.
