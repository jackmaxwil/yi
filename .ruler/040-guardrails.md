# Guardrails

Run `just check` (fmt-check + clippy -D warnings + scripts/guardrails/check_guardrails.sh)
before claiming any task done; quote failures verbatim, do not paraphrase them.

- Ratchets only shrink. Intentional growth is `--update`, in its own commit; a baseline edit in
  the same commit as a code edit fails the build. No aggregate fix, ever. Order is fixed: land
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
- A new YI_* env var is a row in scripts/guardrails/baselines/env_vars.json first (hard cap 40).
- The dist profile is what binary-size and startup budgets measure — never release.
