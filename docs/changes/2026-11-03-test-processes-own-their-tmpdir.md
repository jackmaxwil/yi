---
version: 0.969.0
decisions: D560
raise: tests +7
decision: Run test processes under private TMPDIRs, removed by a wrapper | tests that build dirs under temp_dir() by hand, and runs nextest kills before Drop, left ~96,000 entries (20 GiB) in the user TMPDIR | delete the wrapper-scripts block in .config/nextest.toml and the wrapper calls in justfile
---
`just test`, `just journeys`, and `just dist-suite` now run tests through `scripts/test_tmp.sh`. Nextest gives each test process a private `TMPDIR`; each Cargo-only step has one per invocation. The wrapper creates that directory under the inherited `TMPDIR` (or `/tmp` when unset) and removes it on exit, including on a kill. `scripts/test_tmp_test.py` runs in `just test` and checks a real nextest process and the EXIT trap after SIGTERM. A full suite into an empty TMPDIR left 171 entries before and 0 after.

Merge note: `scripts/guardrails/baselines/binary_size_budget_linux.json` remains removed, matching main's removal of the binary-size gate.
