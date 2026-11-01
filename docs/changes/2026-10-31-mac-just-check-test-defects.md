---
issue: Closes #1961
raise: tests +9
---
`just check` no longer goes red on macOS for three reasons unrelated to the diff under test. The
CLI surface tests clear every provider key from the spawned binary's environment, so a shell's
`OPENROUTER_API_KEY` cannot turn the keyless doctor row into a live probe. Drive tests name their
scratch dir without the test name, which had pushed one socket past the 104-byte `sun_path` limit.
The spared-root sandbox test now spells its spare `/tmp/...` against a root at `/private/tmp/...`,
a real second spelling, where its old guard compared a canonical path with itself.
