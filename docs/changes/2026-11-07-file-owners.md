---
issue: Closes #2086
shares: #2081 both add a guard call to forge_pr.py and a selfcheck line to check_guardrails.sh, at different lines
shares: #2022 it retries runner-failed gate runs in forge_pr.py and pr_review.py; this branch adds the shared stack and diff helpers beside them
---
One open pull request owns a file. `scripts/owners.py` reads every open PR's changed files from its diff against main, and `just pr open` refuses a branch that changes a file another open PR changes, naming both numbers and the files, unless one is stacked on the other or the branch's change file says `shares: #N <why>`. land-fix and ns-lane live outside this repository and are to run `scripts/owners.py check --pr N` or `--paths …` the same way; until they do, `pr open` is the only caller. Change files, ledger lines, ADRs, baselines, AGENTS.md and Cargo.lock own nothing, since the recorder or a ratchet writes them for every branch. A change file's new `shares:` key is checked for its shape.
