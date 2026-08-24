# Guardrails

Run `just check` (fmt-check + clippy -D warnings + scripts/guardrails/check_guardrails.sh)
before claiming any task done; quote failures verbatim, do not paraphrase them.

- Ratchets only shrink. Intentional growth is `--update`, in its own commit; a baseline edit in
  the same commit as a code edit fails the build. No aggregate fix, ever.
- Budgets start at zero — measure the dimension the mess will move to next: glob re-exports 0,
  production duplication 0 (prompts and .md included), panics 0.
- Size ratchets count src/ only; test LOC has its own budget (check_test_size.py --update).
- A new YI_* env var is a row in scripts/guardrails/baselines/env_vars.json first (hard cap 40).
- The dist profile is what binary-size and startup budgets measure — never release.
