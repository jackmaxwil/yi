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
| 2026-08-27 | D48 kitty `o=z`: miniz_oxide 0.9 (`with-alloc`, default features off) + adler2, for compressed orb transmission. Measured against the same tree without it: 5759808 -> 5792880 | 5792880 | 2.36 | 20 / 167 |
| 2026-08-28 | U37 diff bodies + U38 kernel cell + D63 hand-rolled `yi-tui::highlight` (five languages, no dependency): 5991664 -> 6024752, **+33,088 bytes for all three phases**. **syntect was measured and rejected** — a probe binary carrying syntect 5.3 (`default-features = false`, `regex-fancy` + `default-syntaxes` + `dump-load`, 75 syntaxes loading) came to 681,984 bytes against a 286,208-byte do-nothing baseline built beside it: **+0.38 MiB against 0.29 MiB of headroom** under the 6 MiB budget, before its 39 transitive crates. A trimmed dump would have fit the bytes but needs vendored `.sublime-syntax` blobs (`blob_size`) and a `yaml-load` build step | 6024752 | 2.55 | 20 / 167 |
