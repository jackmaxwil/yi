# Build cost (B0)

Measured 2026-11-15 on an Apple M4 Max, 16 cores, 128 GB, toolchain 1.99.1, Rune `feac7b15`, with the workspace as of B0 (empty libraries plus the hello window). Wall clock, one run each unless noted.

Profiles used: `dev` with `debug = "line-tables-only"` and `[profile.dev.package."*"] debug = false` (Yi's and Rune's own setting: no debug info for dependencies); `release` with Rune's `codegen-units = 16`, `incremental = true`.

| Measure | Result |
|---|---|
| Clean debug build, `cargo build --workspace` after `cargo clean` | 96 s |
| Incremental debug build after touching `crates/panes/src/lib.rs` and `crates/app/src/main.rs` | 3.45 s |
| `target/` after the debug build | 2.1 GB |
| `target/debug/` after `just check` (adds clippy and test builds) | 2.8 GB |
| `target/` with debug, `just check` and both release builds | 5.3 GB |
| Debug binary `target/debug/yi-desktop` | 108,652,024 B |
| Release binary, no `tooling` (`cargo build --release -p yi-desktop`, 155 s from a warm debug tree) | 26,496,208 B |
| Release binary with `--features tooling` (74 s after the plain release build) | 28,910,272 B |
| One headless snap, `--scene hello --snap`, debug binary | 0.93 s warm (2.55 s first run, 1.12 s second) |
| One headless snap, release binary with `tooling` | 0.08 s warm (0.51 s first run) |

A lane holding only debug builds sits under 3 GB, so no further cut to debug info was needed: dependencies already carry none.
