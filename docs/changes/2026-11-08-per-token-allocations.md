---
issue: Closes #2111
cut: none crates/ai/src/sse.rs
cut: none crates/loop/src/run.rs
cut: none crates/tui/src/app/stream.rs
cut: none crates/tui/src/markdown.rs
raise: crate acp +58, crate ai +5, crate loop +2, crate runtime +4, crate tui +30, crate types +50, tests +79, comments +7
---
Six hot paths stop building data they throw away, with no change in what any of them sends, stores or draws. The request's stable key hashes a streamed serialization of schema, tools and system instead of a `Value` tree and a 52 KB string: 1,085 allocations per request become 8, and the key is the same bytes D311 recorded, pinned against the first formula on the life-sized request context. A tool looked up by name reads `AgentTool::name` instead of building every tool's schema, about 1,100 allocations per tool call. The daemon relays a worker's plain `session/update` as the line it read, routed by a borrowed view of four fields (`AcpRoute`), and parses the frame only for a state change, a rename, a request or a field its view cannot borrow: 169,403 instructions per streamed delta become 104,426 in a real `yi serve`. The TUI reducer skips the escape parse a seam does not need, refills its live buffers in place and scans the unsettled block without copying it: a text delta costs 15.9 allocations instead of 27.8. A cached pane frame clones no row below the window: 2,574 allocations become 407 scrolled 400 rows up and 12,368 become 379 at the top. The SSE decoder reads each line in place: 20.15 allocations and 4.18 reallocations per event become 16.15 and 0.18.

The message_end rise the red team saw at pace 100 (+177 allocations) is work the reveal moved, not work added: on a clock that steps 2 ms per event message_end costs 92 allocations before and 90 after, at pace 0 and pace 100 alike. On the wall clock faster deltas leave more unrevealed text for message_end to commit, and the message still costs less: its 2,000 text deltas and its message_end take 15,329 allocations, against 29,692 before.
