---
version: 0.1043.0
decisions: D596
issue: Closes #2199
cut: none scripts/pr_review.py
decision: a review round runs on any model family, so `just pr review` no longer refuses the authors' family (`anthropic`) and `AVOID_FAMILIES` and `family()` are deleted | the owner directed that review models should not block on being from the same family, and the refusal stopped a round from running in a session whose own model is Anthropic | restore `AVOID_FAMILIES = ("anthropic",)`, `family()` and the refusal at the top of `cmd_review`
---
`just pr review` no longer refuses to run when the review model is from the authors' family. It used `YI_REVIEW_MODEL`, else the configured primary, and returned 1 with `the review model ... is from anthropic, the authors' family; set YI_REVIEW_MODEL to another` when that was an Anthropic model. A round now runs on whatever model that names. `docs/FORGE.md` and the `pr-review` skill no longer say the model must differ from the authors' family.
