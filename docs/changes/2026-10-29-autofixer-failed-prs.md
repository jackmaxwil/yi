---
issue: Closes #1873
---
Seven open PRs carried `autofix:failed` and the comment beside it named nothing a person could act
on, so the fixer's own failures were read as the author's problem (Refs #1873). Three of them were
the fixer's: a staging write that met another process's `.git/index.lock` in the clone lost the
whole PR (#1779), a fix that changed nothing said only that and never what its model answered
(#1709), and the failure label was set before its comment was posted, so a forge read that failed
in between left a PR labelled with no reason at all (#1732, #1733). A staging write now waits out
the lock, and one that never frees defers the PR instead of failing it; a fix that changed nothing
quotes its model; the comment goes out before the label; and a PR the pass cannot read is that
PR's, so the rest of the pass still runs.

Every stop now names the command a person runs next: `git fetch origin && git merge
origin/<base>` where the conflict is in a file the fixer may not touch, and `just pr autofix N`
after the label is removed where the fixer's own count stopped it. The wall itself is unchanged:
a guardrail file, a baseline, a threshold and a lint level are still files no fix may edit, and a
conflict in one still stops the fixer. What was by design stays by design: four fixes in a row
that did not clear the review (#1694, #1731), the commit hook refusing a fix twice (#1703), and
the twenty-nine failed `autofix` job runs, which are the runner losing its docker daemon, its
image, or the network at checkout and the fgj download, none of which the fixer reaches.
