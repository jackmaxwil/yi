---
version: 0.1001.0
issue: Closes #2016
---
`just pr retry N` now reads each failed gate job's log and reruns only runner-failed runs, at most twice per head. A compiler or test failure, an unreadable log or an unknown failure is never retried. The retry uses Forgejo's whole-run rerun because no single-job rerun is exposed; a run containing both failure classes is refused. `just pr merge` and `just land` use the retry automatically, while `just pr status` names it. Logs come through the existing `just ci-log` path in the infra repository-apply, and a signed PR comment is the per-head retry ledger.
