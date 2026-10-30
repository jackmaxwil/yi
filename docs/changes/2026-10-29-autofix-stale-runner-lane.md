---
issue: Closes #1334
---
The autofixer now merges the base into a PR branch at once, without the two quiet hours, when the base changed a workflow's `runs-on` line since the branch left it and the branch did not touch that workflow. On 2026-10-12 `review.yml` moved to the `llm` runner, ten branches that had not merged main kept running their own copy on `ubuntu-latest`, and those runs held five of the six general runners for nearly an hour. An `autofix:hold` label still keeps the fixer out. The login check in `review.yml` and `autofix.yml` also tries three times, five seconds apart, so an empty reply from `fgj api user` no longer ends the job on a JSONDecodeError traceback; after the third empty reply it fails with the existing "signed in as ''" message.
