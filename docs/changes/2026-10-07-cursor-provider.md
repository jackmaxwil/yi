---
issue: Closes #1115
growth: +1972 the cursor poll and refresh, the connect codec, the http/2 run stream, forwarding each text frame as it arrives, and answering a native shell frame as bash; weighed for deletion: a generated protobuf crate (the frames this client sends are a few field numbers), a second tls stack (the handshake uses the rustls ureq already links), and a second cancel type (the session's fired flag already stops a tool)
decision: provider cursor is compiled, its login poll, refresh URL, and Connect client version live in the binary, and a turn calls Cursor models over HTTP/2 | a profile cannot express the poll or the agent RPC | delete the cursor login, the Connect client, and the catalog file
raise: crate ai +1583, crate loop +57, crate oauth +195, crate runtime +142, crate types +4, tests +239, comments +3
---

`yi login cursor` opens Cursor's deep-control login, polls until the account token arrives, and stores it beside the other provider tokens. A profile at `~/.yi/oauth/cursor.json` still wins. A `cursor` model streams through Connect protobuf on HTTP/2. Each text frame is sent on as it arrives. A tool frame runs the session's tool once, including permission, and the loop does not run that call again. A native shell frame runs bash. The stream closes when the session aborts. A frame this client does not map gets an error result so the turn continues.
