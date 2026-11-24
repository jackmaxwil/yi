---
version: 0.1118.0
raise: comments +2, crate runtime +12, tests +10
---
Yi's own tests no longer read the live machine's load when they start a child. A test process fixes admission to one roomy host sample, the same one admission's own unit tests start from, so a test that spawns a second child passes on a machine at load 130 as on an idle one. The call is hidden from the crate's documentation and takes no argument, so no caller can feed admission any other sample; nothing in the shipped binary makes it, and no flag, environment variable or config key reaches it, so a real session is still admitted by the live load, memory and disk, with the same limits. On a 16-core Mac at load 105 to 175, 62 tests failed on main with `the host has no room for another session`, and the `request_budget` guardrail, which the commit hook runs, failed the same way; with the change, the same runs show the refusal zero times.
