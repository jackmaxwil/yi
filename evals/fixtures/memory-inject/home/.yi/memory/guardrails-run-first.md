---
name: guardrails-run-first
description: Run the repository's own gate before claiming a task done; a green test suite alone is not the gate.
type: feedback
---
`just check` runs fmt, clippy, the guardrails scripts and the tests, and exits on the first red.

**Why:** a landing reported "tests pass" while clippy and the size ratchets were red.

**How to apply:** judge every gate by its exit code, never by a grep over piped output, and quote the failing line verbatim.
