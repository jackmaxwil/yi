---
issue: Closes #996
decision: a review round reports only findings on lines the PR's own patch (its fork to the head) adds, and after a merge from the base it reads the whole PR rather than a range carrying the base's commits; the review job runs main's checkout and reads the PR as data (amends D319 and D322) | the owner: "Reviews are currently commenting on PRs with findings for code outside of PR scope"; and the job ran the PR's own justfile and scripts with yi-bot's token, whose comments count as rounds | restore `judged` and the delta from any clean head, and drop `ref: main` from review.yml
---
A review round judges the PR's change and nothing else (Closes #996). A finding counts only on a line the PR's
own patch adds; one elsewhere is dropped before any refuter runs and the round says how many
went. A later round no longer reads from a clean head the branch has since merged its base into,
which had put the base's new code in front of every lens. The review job checks out main, so a
PR's own scripts, probes and justfile never judge it while it holds yi-bot's token.
