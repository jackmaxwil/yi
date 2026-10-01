---
issue: Refs #996
decision: the review job runs on `pull_request_target`, so its workflow file is always main's, and a schema answer in prose gets one repair turn on the same session before a round is voided (amends D319, D332) | all 9 voided review jobs of 2026-10-01 were a refuter answering in prose ("refuted=false — …", "**Refuted.**"); a `pull_request` run reads the PR's own copy of review.yml, which a PR can edit to pass the required check | return review.yml to `pull_request` and drop the `--continue` repair in `pr_review.ask`
---
Review rounds stop voiding on a refuter that answers in prose: every one of the 9 voided jobs
of 2026-10-01 was a refuter that read the code and replied "refuted=false — …" or "**Refuted.**"
instead of the JSON, because its prompt never showed the JSON and said "answer refuted=false".
The refuter prompt now shows the object, and `pr_review.ask` asks the same session once for
only the JSON before it re-reads the PR. The review job runs on `pull_request_target`, so the
workflow file is main's: a PR can no longer edit the job that judges it, and branches that
never merged main no longer run a stale copy that skipped review.
