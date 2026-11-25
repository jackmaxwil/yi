---
cut: none scripts/guardrails/check_workflows.py
cut: none .forgejo/workflows/postmerge.yml
cut: none .forgejo/workflows/pr.yml
shares: #2363 only the checkout step line in release.yml changes here, apart from its signing steps
---
Every `uses:` step in the forge workflows is fetched from a pull mirror on the forge itself (`the image registry/actions-checkout`, `the image registry/actions-cache`, `the image registry/actions-upload-artifact`, `the image registry/actions-download-artifact`, `the image registry/swatinem-rust-cache`, each synced from github.com hourly) instead of from github.com on every job. Between 10-05 and 11-22, 328 jobs died at set-up because the runner could not reach github.com to fetch `actions/checkout`: 313 in the 11-04 outage and a few every Sunday at 11:00Z. `check_workflows.py` now refuses a step fetched from anywhere but the forge or the repository. The lines it, `pr.yml` and `postmerge.yml` lose are steps rewritten to the mirror URLs, not removed steps or checks.
