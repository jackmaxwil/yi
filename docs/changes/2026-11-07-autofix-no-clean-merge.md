---
version: 0.1000.0
issue: Closes #2088
cut: scripts/pr_autofix.py | push_head(repo, ref, new) | a clean main merge pushed into a PR that is only behind reran its whole gate and voided its review rounds, 41 times in a day on #2022, while the queue merges onto main itself
---
The autofix pass no longer pushes a clean merge of main into a PR that is only behind, unless main changed a workflow's runner line the branch still carries, the stale-lane case where a branch's old copy holds retired runners. It still merges main in a scratch clone, with main's baseline driver, to find a real conflict, and a real conflict still reaches the fixer with its files named. A behind PR whose merge is clean now goes on to the same decision a current one gets, so a draft's carried findings reach the fixer instead of waiting behind a merge that changed nothing. The queue already builds each candidate on main, and the gate fixer already merges main when main explains a red lane, so nothing else was relying on the pushed merge.
