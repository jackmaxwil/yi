---
issue: Closes #1672
raise: crate runtime +41, crate tools +35, tests +57, comments +4
---
A process an outer sandbox already contains could not apply Yi's own Seatbelt profile: macOS refuses to nest, so every contained bash call and every kernel boot died on `sandbox-exec: sandbox_apply: Operation not permitted`. About 67 runtime tests failed that way under the workspace sandbox while passing on the host, and reviewers could not tell those reds from real ones.

`Sandbox::available` now probes once per process whether a profile applies and says so on stderr when it does not. Nested, Yi's Seatbelt is off: a bash call the gate would have contained asks the user (the classifier cannot settle that ask, and yolo refuses the call outright), the kernel runs under the outer profile, and the tests that need Seatbelt return early. On the host the answer is unchanged.
