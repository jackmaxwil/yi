---
issue: Closes #1531
---
The `fetch(url)` and `bash(cmd)` contract of the kernel-side `rlm` package ran as an 80-line Python string inside `crates/runtime/tests/kernel_data_surface.rs`, spawned through the machine's `python3`. It is now `python/yi_runtime/tests/test_rlm_bash_fetch.py`, which the unittest lane in `check_guardrails.sh` already runs, with the same assertions: the `kernel://` owner and child-kernel resolution, the empty-command and empty-url refusals, the spawn/tail/poll/kill/release sequence, and the silence of a released handle. Tests sit outside the runtime identity hash, so no venv rebuild follows. `every_rlm_name_the_model_reads_is_public_in_the_package` stays in Rust because it reads Rust-side text.
