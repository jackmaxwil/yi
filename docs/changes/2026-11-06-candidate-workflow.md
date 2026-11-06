---
issue: Closes #2042
---
The merge queue's candidate gate now runs from its own workflow, candidate.yml, which names the `queue` runner outright. The `runs-on` choice by branch that #2035 added never took effect: Forgejo evaluates `runs-on` with no `github` context, so every candidate job still landed on `ubuntu-latest`. pr.yml runs on pull requests only again, and the workflow check refuses a candidate.yml whose env or gate steps differ from pr.yml's, so the gate the queue lands on stays the one every PR passed.
