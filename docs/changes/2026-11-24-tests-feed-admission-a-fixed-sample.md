---
raise: comments +2, crate runtime +9, tests +27
---
Yi's own tests no longer read the live machine's load when they start a child. A test process hands admission one fixed, roomy host sample (`admission::fix_sample`), so a test that spawns a second child passes on a machine at load 130 as on an idle one. Nothing in the shipped binary sets that sample: no flag, no environment variable and no config key reaches it, so a real session is still admitted by the live load, memory and disk, with the same limits. On a 16-core Mac at load 105 to 175, 62 tests failed on main with `the host has no room for another session`, and the `request_budget` guardrail, which the commit hook runs, failed the same way; with the change, the same runs show the refusal zero times.
