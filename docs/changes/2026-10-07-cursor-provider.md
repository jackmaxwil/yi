---
issue: Closes #1115
growth: +1179 the cursor poll and refresh, the connect codec, and the http/2 run stream; weighed for deletion: a generated protobuf crate (the frames this client sends are a few field numbers) and a second tls stack (the handshake uses the rustls ureq already links)
raise: crate ai +939, crate loop +1, crate oauth +184, crate runtime +55, tests +172
decision: provider cursor is compiled, its login poll, refresh URL, and Connect client version live in the binary, and a turn calls Cursor models over HTTP/2 | a profile cannot express the poll or the agent RPC | delete the cursor login, the Connect client, and the catalog file
---

`yi login cursor` opens Cursor's deep-control login, polls until the account token arrives, and stores it beside the other provider tokens. A profile at `~/.yi/oauth/cursor.json` still wins. A `cursor` model streams through Connect protobuf on HTTP/2. Text comes back as the usual assistant events. A tool frame runs the session's tool once, including permission, and the loop does not run that call again. A frame this client does not map gets an error result so the turn continues.
