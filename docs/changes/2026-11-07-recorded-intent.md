---
issue: Closes #2082
---
A change file can now record what it removes or holds on purpose: `cut: <path> | <the deleted line> | <why>` and `keep: <path> | <line> | <why>`. `scripts/guardrails/check_intent.py` reads every landed change file's records at the branch's fork and refuses a branch, by any author, that puts a cut line back or deletes a kept one, naming the change file that recorded it; a branch overturns one with `revives:` or `drops: <change file> <path>` beside a `decision:`. It runs with the guardrails, so on every commit hook, push and queue candidate. `just pr open` refuses a branch that deletes five or more substantive lines from a file on net without a `cut:` line for it, or `cut: none <path>`.
