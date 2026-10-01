# Guardrails

Run `just check` (fmt-check + clippy -D warnings + scripts/guardrails/check_guardrails.sh)
before claiming any task done; quote failures verbatim, do not paraphrase them.

- The size ceilings — net src growth, per-crate src lines, test LOC, comment volume and
  over-cap count — are measured at the fork point (`git merge-base origin/main HEAD`), not stored:
  growth past the fork's number is a `raise:` line in the branch's change file (`tests +N`,
  `comments +N`, `over-cap +N`, `crate <name> +N`), and a shrink on main tightens the next branch
  with no commit. Two branches never write one shared ceiling, which is what made every PR conflict.
- Every other ratchet only shrinks. Intentional growth is `--update`, in its own commit; a baseline edit in
  the same commit as a code edit fails the build (check_commit_style refuses the mix per commit at the PR range). No aggregate fix, ever. Order follows direction (097): a raised
  ceiling's `--update` commit lands before the code, since guardrails --fast runs on every
  commit and refuses one against a ceiling it would exceed; a shrunk ceiling's `--update`
  commit lands after, since a shrink-only check is never red.
- A guardrail can fail on a file that is not yours: `blob_size` fires on any untracked blob in
  the tree. Report it, do not allowlist or delete another session's artifact.
- Budgets start at zero — measure the dimension the mess will move to next: glob re-exports 0,
  production duplication 0 (prompts and .md included), panics 0.
- Size ratchets count src/ only; test LOC has its own budget (check_test_size.py, `raise: tests +N`).
- Net src growth is priced per branch, against its fork point: +150 lines ride free; past that the
  branch's change file carries `growth: +N <memo>` naming the measured number and what was weighed
  for deletion; past +2000 that file also carries the `decision:` the landing claims.
- Growth is paid for before it is excused. The budget is a price, not a permission: a landing
  that cannot say why its bytes earn their place does not land, and deletion is weighed first.
  The memo's number is checked against the measurement, trailing it by at most the free band.
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
