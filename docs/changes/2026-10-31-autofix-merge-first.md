---
issue: Closes #1944
raise: crate acp +9, tests +53, comments +1
---
A red test that passes once the base is merged in now takes that merge, with no model turn, because the head only lacks what the base landed. The fixer tries the merge in a scratch worktree and pushes it only when the failing tests are red there before it and green after, so a flake or a head merely behind a busy main still goes to the model.

The daemon now runs settings requests in the order they arrive. Saves ran side by side, so a quick double press of the sidebar chord could leave the older state in `config.json`, and the console would open on it next time.
