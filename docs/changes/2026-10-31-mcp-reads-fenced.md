---
raise: crate runtime +13, tests +106, comments +2
---
`read` and `grep` on an `mcp://` address now hand the model the server's text inside the same `yi-external` data fence a web search arrives in, with `mcp://<server>/<resource>` as its source and the session's screen applied. Before, the server's JSON came back bare, so a hostile resource could not be told from the agent's own output. A program's `fetch` of the same address keeps the bare text, since code parses it; `yi mcp` output in a cell or bash is a separate seam this change leaves alone.
