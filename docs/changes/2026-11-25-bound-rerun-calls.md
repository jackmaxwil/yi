---
cut: none scripts/forge_pr.py
cut: none scripts/pr_autofix.py
---
A gate rerun (`forge_pr.cmd_rerun`, which closes and reopens a pull request) bounds every forge call: reads and reopens 15 s, the close 60 s. A read before the close that does not answer means nothing reran; a close refused with the pull request open returns failure with no reopen; a close that did not answer is waited out before the reopen, and a pull request never ends closed without a message saying so. The git fetch in `pr_review.holds_for` has a timeout. The fixer's ledger and comment posts share one poster, and a title tidy that fails no longer ends the fixer's pass. The lines `scripts/forge_pr.py` and `scripts/pr_autofix.py` lose are the rewritten rerun path and the folded post helpers, not removed behaviour.
