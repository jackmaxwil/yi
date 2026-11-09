---
version: 0.1010.0
issue: Closes #2101
raise: tests +26
---
A `wait_s` of `1e30` or any other huge number passed to `tree.check` waits the 590 second maximum instead of panicking inside the standard library, which under the dist profile's `panic = "abort"` killed the whole `yi acp` worker for that root. A negative or non-numeric `wait_s` still takes the 30 second default.
