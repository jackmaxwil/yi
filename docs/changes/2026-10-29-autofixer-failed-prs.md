---
version: 0.932.0
issue: Closes #1873
---
Seven open PRs carried `autofix:failed` and the comment beside it named nothing a person could act
on (Refs #1873). Three of them were the fixer's own: a model turn that timed out was killed but its
children were not, so a git it left running held the clone's `.git/index.lock` and lost the PR
(#1779); a fix that changed nothing said only that and never what its model answered (#1709); and
the failure label was set before its comment was posted, so a forge read that failed in between left
a PR labelled with no reason at all (#1732, #1733).

A timed-out turn now takes its whole process group with it. A fix that changed nothing quotes its
model. The comment goes out before the label. A PR the pass cannot read is that PR's, so the rest
of the pass still runs, and it counts toward the pass's quota because its fix may have paid. A
conflict in a file the fixer may not touch now names `git fetch origin && git merge origin/<base>`,
and the deferred line no longer blames the provider for a held lock or a full disk.
