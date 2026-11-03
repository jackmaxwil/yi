---
decision: Run every nextest process under its own TMPDIR, removed by a wrapper | tests that build dirs under temp_dir() by hand, and runs nextest kills before Drop, left ~96,000 entries (20 GiB) in the user TMPDIR | delete the wrapper-scripts block in .config/nextest.toml
---
`just test` now wraps each test process in `scripts/test_tmp.sh`, which gives it a private `TMPDIR` under `/tmp` and removes it on exit, including on a nextest kill. A full suite into an empty TMPDIR left 171 entries before and 0 after. The plain `cargo test` fallback is not wrapped.
