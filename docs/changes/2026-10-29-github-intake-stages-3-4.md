---
version: 0.895.0
issue: Refs #1719
ledger=: GitHub-to-forge intake | 22 | gated manual | H | H | stages 0–4: real fixtures, signed ordered checks, read-only dry-run, deterministic draft import and a no-tools lens; an imported PR carries `external` and waits for the owner's `run-ci`, which asks for the disposable `external` runner (unregistered, so an admitted job queues and never runs); landing comments `landed as <sha>` and closes the GitHub PR; the poll workflow is inert (cron commented)
ledger~: verb table, `yi --help` and completions | 33 | 34
---
GitHub intake stages 3 and 4 hold contributor code behind the owner's `run-ci`
label and finish the loop on the GitHub side. An imported PR already carried
`external` and `autofix:hold`; now `pr.yml`'s `gate` and `title` jobs and
`plugins-live.yml` also require `run-ci`, ask for a disposable `external` runner,
and skip the autofixer, `live` and bench jobs. No runner registers that label
yet, so an admitted job queues and contributor code never runs: the design's
fail-closed default. The hold is not bypassable from the head because intake
denies every `.forgejo/` path and the import refuses to push unless the imported
tree's `.forgejo/` is byte-equal to current main. The `title` job holds the repo
token, so an external PR runs main's `scripts/` there.

`just intake --run-ci <N>` admits a reviewed external PR: owner only, external
only, an open PR, its audited imported commit still the same change (a base-only
merge is allowed, a moved change is not) and `.forgejo/` equal to current main.
`just intake --land <N>` comments `landed as <mirror-sha> on Forgejo` on the
GitHub PR, gives the one-line rebase instruction and closes it, mapping the forge
head through the mirror's commit map. Both run behind the transport seam and are
tested with the fake transport only; the real transport's write half is not
exercised here. `just intake --poll` is the write pass, while `.forgejo/workflows/intake.yml`
runs the read-only one and is inert: `workflow_dispatch` only, its cron line
present and commented, naming no secret.

`docs/FORGE.md` gains the `external` runner, the hold's argument and a "GitHub
contributors" section for the owner and Max; the runner requirements live in the
ops handoff at `2026-10-29-intake34/ops-handoff.md` in the design directory. Max's
login, email and key are left for the owner to add.
