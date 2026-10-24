---
issue: Closes #1466, Refs #1442
---
This PR carries two units of Yi's next shape, E-ci-live-weekly and HD-G02-surface-trims.

The paid weekly live job, `live-weekly`, moves out of `.forgejo/workflows/tracking-hygiene.yml` into its own `.forgejo/workflows/live-weekly.yml`. The job is the three flash runs, the Anthropic run and the OpenAI run, with its 40 minute timeout, rust-cache key and per-secret skips unchanged. The new workflow keeps the weekly Monday 05:17 UTC trigger the job already ran on and adds `workflow_dispatch`, which tracking-hygiene already had; the trigger now exists in each file for its own jobs only, so the paid job runs once a week. tracking-hygiene.yml keeps its milestone, hygiene and catalog-drift jobs and its header about milestone dates. The one-routing-pin check in `evals/selftest.py` (the ledger judges a PR's live run by main's runs of the same routing) now reads `live-weekly.yml` where it read tracking-hygiene.yml, since the `EVAL_ROUTING` pin moved with the job. docs/FORGE.md lists the new file and names it as the reader of the three provider keys.
