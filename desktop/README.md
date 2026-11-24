# Yi Desktop

A native Mac app for Yi, drawn with Rune. It lives in Yi as `desktop/`, its own cargo workspace (Yi's `docs/YI_DESIGN.md` §17.5); the staging repository it was built in keeps the per-task history.

- Layout: six crates under `crates/` (`yd-core`, `yd-sources`, `yd-ui`, `yd-panes`, `yd-mac`, `yi-desktop`); Rune is the owner's fork at a pinned git revision; `yi-types` and `yi-browser` are Yi's own crates, by path (`../crates/types`, `../crates/browser`).
- Build: `cargo build -p yi-desktop` (toolchain 1.99.1 from `rust-toolchain.toml`); run with `cargo run -p yi-desktop`.
- Headless: `target/debug/yi-desktop --scene shell/stub --snap hello.png` renders a scene; `--tree tree.json` dumps its element tree; `--list-scenes` lists them. Release builds need `--features tooling` for these flags.
- Gates: `just check` (format, clippy, HAR, tests, scenes); `just supply` runs cargo-deny and cargo-audit. The forge has no Mac: its `desktop` lane runs `just gate`, which is `check` without the scenes, on Linux, and a test that only a Mac can pass says so in its `ignore`. Scenes and journeys run on a Mac before a change lands.
- Lanes: `just lanes add N | reset N | status` manages worktrees at `../lN`.
- Notes: `docs/rune-notes.md` (Rune facts), `docs/deps.md` (every dependency), `docs/build-cost.md` (build numbers).
