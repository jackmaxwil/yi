---
issue: Closes #1896
decision: the autofix pass reads every open pull request page by page, and its limits delay a fix instead of parking the PR: $16 of spend on a PR's last 24 hours, $60 a day, 6 review fixes and 3 gate fixes in a row, 5 fixes a pass, a 90-minute job, a label that clears itself after 12 hours twice, and a git lock read as a transient one | on 2026-10-30 102 pull requests were open and a pass read page one's 50, so the oldest 52 were never fixed; seven PRs sat on `autofix:failed` from those limits (four review fixes in a row, a per-PR cap, a git index.lock) with nothing saying when they would move again, and the owner is not the person those limits named; the numbers replace the owner's 2026-09-30 set ($8, $25, 4 review fixes, 2 gate fixes, 3 fixes a pass, 45 minutes) | restore the constants in scripts/pr_autofix.py, the single page in forge_pr.open_pulls and `timeout-minutes: 45` in autofix.yml
---
The autofixer now sees every open pull request and treats each limit as a delay. Its listing pages `pulls?state=open` until a page comes back short, so a PR past the first 50 is fixed like any other; `pr_review`'s intake and sweep read the same helper.

Its numbers are the spend of a PR's last 24 hours ($16), a day's spend ($60), six review fixes and three gate fixes in a row, five fixes a pass and a 90-minute job, and the gate streak's limit is one constant instead of a literal.

A spend limit waits: over the per-PR cap the pass skips the PR and posts one comment per 24-hour window naming the cap and the hour the window reopens, which is when enough of it has aged out that the spend falls back under the cap, with no `autofix:failed`. The day's cap names the midnight it reopens.

`autofix:failed` expires. A pass clears the label 12 hours after the stop that set it — a failure, or the per-PR cap an older pass parked the PR under — twice per PR, and the failure comment says which: the hour it clears itself, or "no more automatic retries, a person is next". The streak restarts from the last failure, so the next fix counts from zero.

A git `Unable to create …​.lock` is a transient error now, so an attempt that meets one is deferred like a 429 rather than labelled failed.
