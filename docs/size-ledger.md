# Size ledger (§13.6)

Measured on macOS arm64, rustc 1.94.0, profile `dist`. Candidate-dependency deltas (ureq vs
reqwest, jiff vs chrono, globset vs regex-lite) are recorded here before each choice is final;
the §13.3 table is the expected outcome, this ledger wins if they disagree.

| date | change | dist binary (bytes) | `yi --version` (ms) | deps direct/transitive |
|---|---|---|---|---|
| 2026-08-22 | phase 0 scaffold: empty 13-crate workspace, no external deps | 286064 | 1.4 | 0 / 0 |
| 2026-08-24 | phase 1: tokio(rt,sync,time,macros) + ureq2(tls,native-certs) + lexopt + serde stack, full loop/providers/runtime/cli | 1614528 | 2.3 | 5 / 68 |
| 2026-08-24 | phase 2 (session+tools): + globset (pulls regex-automata, aho-corasick), yi-session + yi-tools + adapters in the binary | 2229856 | 2.3 | 7 / 75 |
| 2026-08-24 | D34 OpenRouter: data/openrouter.json (139 KB, 349 models) baked into the catalog + yi rpc surface | 2693296 | 2.3 | 7 / 75 |
| 2026-08-24 | phase 2b: hashline (xxhash-rust xxh32) + yi-permission (sha2) + prompt.md in the edit tool | 2858992 | 2.1 | 9 / 83 |
| 2026-08-24 | 2c/D36 MCP compiled-in: rmcp 3.1.4 (client + child-process transport, default-features off; chrono transitively — wrapper exception) + yi-mcp-cli in every build | 3853616 | 3.9 | 10 / 107 |
| 2026-08-24 | 2c batch 2 (D37): streamable-HTTP client feature of rmcp (sse-stream, futures-util, http as type-level directs) + OAuth over ureq | 4151984 | 2.4 | 13 / 111 |
| 2026-08-24 | phase 4 (kernel): zeromq 0.6 pure-Rust (rand/regex/dashmap internals, deny-wrapped) + hmac; yi-kernel + ipython + subagent host in the binary | 4666400 | 3.8 | 15 / 132 |
| 2026-08-24 | phase 7 (D41, §13.4 `tui`): ratatui 0.29 (crossterm, scrolling-regions) + tui-textarea + pulldown-cmark + unicode-width; `yi-tui` in the default build (yi-cli feature `tui`, default on per X1) — +0.87 MiB, inside the §8.14 ≤ 1 MiB budget | 5577424 | 2.3 | 19 / 165 |
