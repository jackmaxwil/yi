---
version: 0.1080.0
decisions: D608
issue: Closes #2326
cut: none scripts/pr_autofix.py
decision: the fixer answers a blocked round on a ready pull request that is not in the queue, and still leaves a queued one alone (revises D320's draft-only fixer) | a ready pull request with a blocked round was nobody's job: the fixer skipped it as ready and the queue refused it as blocked, so #2237 and #2299 sat for a week; a push restarts checks only on a landing, and a landing carries the `queue` label | drop the `queue` and verdict clause in `attempt` of scripts/pr_autofix.py, so every ready title clears `blocked` again
---
Main had not moved since 2026-11-19 with every bot healthy, because each open pull request sat in a state nothing left. Four causes are fixed here. The queue tick now re-points the open pull requests based on a landed branch at main and reruns their checks, so a stack opened on its own branches lands from the bottom up instead of being refused as "not based on main" for good. The review bot's duplicate rule no longer counts lockfile lines, and it skips a pull request's whole chain of bases rather than one hop, which clears the false twins on #2297 and #2321. The fixer answers a blocked round on a ready pull request outside the queue. Its description tidy reads `closes #N` in any case, as the title check does. `Cargo.lock` takes `url` 2.5.9, which clears RUSTSEC-2026-0592: that advisory had failed the guardrails lane on every run since 2026-11-22.
