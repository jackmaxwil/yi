---
issue: Closes #1214
raise: tests +55
---
A failing test now leaves something to read. When an integration test panics, its scratch directory (frames, casts, session files, the fake HOME) is copied to `target/tmp/failed/<name>` before the temp copy is removed, and the test's stderr names the path. The `pr` workflow's test lane uploads that directory as the `failed-tests` artifact for seven days when the lane fails, so a drive test that flakes on the forge can be read from its frames rather than its log. A test that fails by returning an error rather than panicking is not kept.
