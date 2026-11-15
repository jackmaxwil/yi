---
version: 0.1063.0
issue: Refs #2160
cut: none scripts/pr_review.py
growth: +0 a binary-age check in the reviewer's binary lookup, with its selfcheck
---
The review bot runs the first `yi` it finds under `target/`. A `target/dist/yi` built on 2026-11-04 outlived the config schema, rejected `providers` in `~/.yi/config.json`, and voided every round on 2026-11-14. The lookup now skips a binary older than the newest Rust source or manifest in the checkout and rebuilds it. A pinned `YI_BIN` is left alone.
