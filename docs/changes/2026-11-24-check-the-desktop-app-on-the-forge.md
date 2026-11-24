---
issue: Closes #2341
cut: none desktop/justfile
shares: #2343 this adds one lane name to `GATE_LANES` in pr_review.py and a word to one docstring in pr_autofix.py, lines that pull request leaves alone
shares: #2342 this changes the candidate's stale limit and its comment in pr_queue.py and one self-check's lane list in forge_pr.py, lines that pull request leaves alone
shares: #2239 this changes the `yd-mac` row of the crate table in desktop/docs/architecture.md, eighty lines above the one that pull request changes
decision: Yi Desktop is gated on the forge as a Linux build: a `desktop` lane beside `lint`, `guardrails` and `test` runs the app's format, lint, HAR and test gates, while its scenes and journeys stay on a Mac | the forge has no Mac, so 34 bot commits to `desktop/` in three weeks landed uncompiled and three of them left main's desktop tests red; a macOS guest on the Intel host could not draw, and the app builds on Linux once `yd-mac` is Mac-only | take `desktop` out of the gate matrix in pr.yml and candidate.yml and out of `GATE_LANES`
---
The forge now builds and tests Yi Desktop. A fourth gate lane, `desktop`, runs the app's own format, lint, HAR and test gates on Linux for a pull request that touches `desktop/` or the Yi crates the app builds on, and says so and passes at once for one that does not. The merge queue's candidate runs it too, and the fixer reruns it like any red lane, so a bot's change to the app is compiled and tested before it lands. The app's menu-bar crate is a stub off the Mac, tests press ⌘ the way each system reports it, and eighteen tests that only a Mac can pass say why in their `ignore`. The 211 scenes and 19 journeys still run on a Mac before a desktop change lands. #2341
