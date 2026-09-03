# Guardrails

Run `just check` (fmt-check + clippy -D warnings + scripts/guardrails/check_guardrails.sh)
before claiming any task done; quote failures verbatim, do not paraphrase them.

- Ratchets only shrink. Intentional growth is `--update`, in its own commit; a baseline edit in
  the same commit as a code edit fails the build (check_commit_style refuses the mix per commit at the PR range). No aggregate fix, ever. Order is fixed: land
  the code commit red on the baselines, then `--update` and commit the baselines alone.
- A guardrail can fail on a file that is not yours: `blob_size` fires on any untracked blob in
  the tree. Report it, do not allowlist or delete another session's artifact.
- Budgets start at zero — measure the dimension the mess will move to next: glob re-exports 0,
  production duplication 0 (prompts and .md included), panics 0.
- Size ratchets count src/ only; test LOC has its own budget (check_test_size.py --update).
- Net src growth is priced per version: +150 lines ride free; past that the version's own
  changelog row carries a `growth +N:` memo naming the measured number and what was weighed for
  deletion; past +2000 that row also cites the D-row the landing claimed. check_growth.py reads
  baselines/src_loc.json, and its `--update` obeys the own-commit law like every other baseline.
- Growth is paid for before it is excused. The budget is a price, not a permission: a landing
  that cannot say why its bytes earn their place does not land, and deletion is weighed first.
  `--update` charges the same price before it absorbs a delta, so the baseline update is not a
  way around the memo; the memo's number is checked against the measurement, trailing it by at
  most the free band.
- A new guardrail script's `--selfcheck` is its own refute pass: each check is disabled in turn
  and the selfcheck must fail for that check's reason. The 0.119.0 refute pass was run by hand
  and killed fifteen mutants; the flag is the same pass on every run.
- Every baseline has a reader (check_orphans.py). A new baseline seeds with its gate in one
  commit — the carve-out check_commit_style already grants — and goes when its gate goes.
- A new YI_* env var is a row in scripts/guardrails/baselines/env_vars.json first (hard cap 40).
- The dist profile is what binary-size and startup budgets measure — never release.
- No tracking check runs offline. A green `just check` says nothing about whether the work is
  registered, sized or dated — it cannot reach the forge and never tries. The PR job
  (check_pr_metadata.py) and the weekly hygiene job are where that is enforced.
