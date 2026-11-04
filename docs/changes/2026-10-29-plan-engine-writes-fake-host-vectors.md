---
version: 0.902.0
issue: Closes #1566
raise: tests +36
---
The Python fake host (`python/yi_runtime/tests/fake_host.py`) no longer keeps its own copy of the plan engine's step table, retry cap and label cap. `crates/runtime/tests/plan_vectors.rs` writes `python/yi_runtime/tests/vectors/plan_steps.json` from `STEPS`, `RETRY_CAP` and `TODO_LABEL_MAX` (`YI_BLESS=1` regenerates it) and fails when the committed file differs; the fake reads that file. The fake now refuses `add_edge` on a running todo as the engine does, where it used to apply it in any state (#1566).
