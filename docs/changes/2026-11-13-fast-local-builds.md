---
version: 0.1111.0
growth: +60 build config, docs and an unwired experiment; nothing in src/ moves, nothing was weighed for deletion because every line is new opt-in tooling
---
Opt-in fast local builds. `just dev-fast [cargo build args]` (default `-p yi-cli`) builds with nightly's Cranelift codegen backend on the dev profile into `target/dev-fast`, leaving `target/debug`, `rust-toolchain.toml` (1.94.0) and CI alone. It needs `rustup component add rustc-codegen-cranelift-preview --toolchain nightly` and says so when the component is missing. The recipe sets `RUSTC_WRAPPER=sccache` when sccache is on `PATH`, and on Linux adds `-C link-arg=-fuse-ld=mold` when mold is on `PATH`. mold has no Mach-O support, so macOS keeps the default ld-prime linker. A machine with none of these tools still builds. The recipe is the one entry point: it passes `-Zcodegen-backend` and sets `CARGO_PROFILE_DEV_CODEGEN_BACKEND=cranelift` itself, so no `.cargo` file and no cargo alias exist.

`bacon.toml` points bacon's `clippy` and `test` jobs at `just clippy` and `just test`, so a watch loop runs the suite in the lane home the recipe sets up, never in the real `~/.yi`. `experiments/subsecond/` is a standalone project for Dioxus subsecond hot-patching with its own README, excluded from the workspace via `exclude = ["experiments"]` and not part of any build or gate.

Measured on one Apple Silicon Mac, `cargo build -p yi-cli`, dev profile, wall clock, one run each:

- clean: stable 46.1 s, `just dev-fast` 40.2 s
- incremental, one-line edit in `crates/types/src/lib.rs`: stable 29.0 s then 24.2 s, Cranelift 22.8 s then 22.2 s

That is 13% faster clean and 6 to 21% faster incremental. The incremental time is dominated by rebuilding dependent crates and relinking, not by codegen of the edited crate.
