---
issue: Closes #1214
raise: tests +72
---
A failing test now leaves something to read. Every console drive run keeps its frames at `target/tmp/drive/<name>`, replacing the last run's copy. That covers the two shapes the known flakes fail in: a timed-out wait that returns an error, and an assertion on a frame read after the run's scratch dir is gone. Any integration test that panics also keeps a copy of its scratch dir at `target/tmp/failed/<name>`, and its stderr names the path. A copy that fails partway says so and leaves nothing behind. The `pr` workflow's test lane uploads both directories as the `failed-tests` artifact for seven days when the lane fails.
